import test from 'node:test'
import assert from 'node:assert/strict'
import { createPinia, setActivePinia } from 'pinia'
import { Window } from 'happy-dom'
import type { Project } from '../src/types.ts'
Object.defineProperty(globalThis, 'window', { value: new Window(), writable: true })
Object.assign(globalThis, { isTauri: true })
Object.assign(window, {
  isTauri: true,
  __TAURI_INTERNALS__: {
    invoke: async () => {
      throw new Error('No fixture')
    },
    transformCallback: () => 1,
    unregisterCallback: () => {},
  },
})
const { useChatStore, projectKey } = await import('../src/stores/chat.ts')
const project: Project = {
  id: 'p',
  connectionId: 'a',
  name: 'Project',
  path: '/app',
  description: '',
  defaultAgent: 'fake',
  defaultModel: 'fake-default',
  permissionMode: 'default',
  pinned: false,
  archived: false,
}
const session = {
  id: 's',
  projectId: 'p',
  agent: 'fake',
  title: 'Session',
  model: 'fake-default',
  permissionMode: 'default',
  status: 'idle',
  createdAt: '2026-10-26T00:00:00Z',
  updatedAt: '2026-10-26T00:00:00Z',
}
let cleanup = () => {}
test.afterEach(() => cleanup())
test('unknown delivery is retained per session and is never automatically resent', async () => {
  setActivePinia(createPinia())
  const store = useChatStore()
  cleanup = () => store.$dispose()
  store.sessions[projectKey(project)] = [session]
  const current = store.state(project, 's')
  current.draft = 'original'
  const calls: string[] = []
  window.__TAURI_INTERNALS__.invoke = async (command: string) => {
    calls.push(command)
    throw { code: 'E_DELIVERY_UNKNOWN', stage: 'run.send', message: 'Unknown' }
  }
  await store.send(project, 's')
  assert.equal(current.pending[0].status, 'unknown')
  assert.equal(current.draft, '')
  assert.equal(calls.length, 1)
  const second = store.state(project, 'other')
  second.draft = 'separate draft'
  assert.equal(store.state(project, 's').pending[0].text, 'original')
  assert.equal(store.state(project, 'other').draft, 'separate draft')
  await new Promise((resolve) => setTimeout(resolve, 10))
  assert.equal(calls.length, 1)
})
test('mismatched response targets cannot mark a message delivered', async () => {
  setActivePinia(createPinia())
  const store = useChatStore()
  cleanup = () => store.$dispose()
  store.sessions[projectKey(project)] = [session]
  store.state(project, 's').draft = 'hello'
  window.__TAURI_INTERNALS__.invoke = async (_command: string, args: any) => ({
    target: { ...args.target, sessionId: 'wrong' },
    data: { runId: 'r' },
  })
  await store.send(project, 's')
  assert.equal(store.state(project, 's').pending[0].status, 'failed')
  assert.match(store.state(project, 's').error, /E_STALE/)
})
test('a late saved draft cannot replace new input and flush uses the owning session', async () => {
  setActivePinia(createPinia())
  const store = useChatStore()
  cleanup = () => store.$dispose()
  store.sessions[projectKey(project)] = [session]
  let resolveDraft: (text: unknown) => void = () => {}
  const writes: any[] = []
  window.__TAURI_INTERNALS__.invoke = async (command: string, args: any) => {
    if (command === 'session_draft_get')
      return new Promise((resolve) => {
        resolveDraft = (text) => resolve({ target: args.target, data: text })
      })
    if (command === 'session_draft_set') {
      writes.push(args)
      return { target: args.target, data: null }
    }
    if (command === 'session_history')
      return { target: args.target, data: { events: [], hasMore: false } }
    if (command === 'session_subscribe')
      return { target: args.target, data: { subscriptionId: 'sub' } }
    throw new Error(command)
  }
  const selecting = store.select(project, 's')
  store.state(project, 's').draft = 'new input'
  resolveDraft('old saved draft')
  await selecting
  await Promise.resolve()
  assert.equal(store.state(project, 's').draft, 'new input')
  await store.flushDrafts()
  assert.equal(writes.at(-1).text, 'new input')
  assert.equal(writes.at(-1).target.sessionId, 's')
  assert.equal(writes.at(-1).target.connectionId, 'a')
})
test('draft writes are serialized so a slow old write cannot overwrite a new draft', async () => {
  setActivePinia(createPinia())
  const store = useChatStore()
  cleanup = () => store.$dispose()
  const writes: string[] = []
  let release: () => void = () => {}
  window.__TAURI_INTERNALS__.invoke = async (command: string, args: any) => {
    assert.equal(command, 'session_draft_set')
    writes.push(args.text)
    if (args.text === 'one')
      await new Promise<void>((resolve) => {
        release = resolve
      })
    return { target: args.target, data: null }
  }
  const current = store.state(project, 's')
  current.draft = 'one'
  await new Promise((resolve) => setTimeout(resolve, 320))
  current.draft = 'two'
  await new Promise((resolve) => setTimeout(resolve, 320))
  assert.deepEqual(writes, ['one'])
  release()
  await store.flushDrafts()
  assert.deepEqual(writes, ['one', 'two'])
})
test('temporary disconnection keeps its subscription while an explicit release permits safe resubscription', async () => {
  setActivePinia(createPinia())
  const store = useChatStore()
  cleanup = () => store.$dispose()
  store.sessions[projectKey(project)] = [session]
  const channels: any[] = [],
    owners: any[] = []
  let subscriptions = 0
  window.__TAURI_INTERNALS__.invoke = async (command: string, args: any) => {
    const reply = (data: unknown) => ({ target: args.target, data })
    if (command === 'session_draft_get') return reply('')
    if (command === 'session_history') return reply({ events: [], hasMore: false })
    if (command === 'session_subscribe') {
      channels.push(args.onEvents)
      owners.push(args.target)
      return reply({ subscriptionId: 'sub-' + ++subscriptions })
    }
    if (command === 'session_unsubscribe') return reply(null)
    throw new Error(command)
  }
  await store.select(project, 's')
  channels[0].onmessage({ target: owners[0], events: [], disconnected: true })
  await store.select(project, 's')
  assert.equal(subscriptions, 1)
  await store.releaseConnection('a')
  await store.select(project, 's')
  assert.equal(subscriptions, 2)
  channels[0].onmessage({
    target: owners[0],
    events: [
      {
        seq: 100,
        sessionId: 's',
        runId: 'stale',
        ts: '2026-10-26T00:00:00Z',
        type: 'run.started',
        payload: { state: 'running' },
      },
    ],
  })
  assert.equal(store.state(project, 's').runId, undefined)
  channels[1].onmessage({
    target: owners[1],
    events: [
      {
        seq: 10,
        sessionId: 's',
        runId: 'new',
        ts: '2026-10-26T00:00:00Z',
        type: 'run.started',
        payload: { state: 'running' },
      },
    ],
  })
  assert.equal(store.state(project, 's').runId, 'new')
  assert.equal(store.state(project, 's').error, '')
})

test('switching sessions releases the previous server subscription and rejects its late channel', async () => {
  setActivePinia(createPinia())
  const store = useChatStore()
  cleanup = () => store.$dispose()
  store.sessions[projectKey(project)] = [session, { ...session, id: 'other' }]
  store.state(project, 's').loaded = true
  store.state(project, 'other').loaded = true
  const calls: { command: string; args: any }[] = []
  window.__TAURI_INTERNALS__.invoke = async (command: string, args: any) => {
    calls.push({ command, args })
    return {
      target: args.target,
      data:
        command === 'session_subscribe' ? { subscriptionId: 'sub-' + args.target.sessionId } : '',
    }
  }
  await store.select(project, 's')
  await store.select(project, 'other')
  assert.equal(store.state(project, 's').subscriptionId, undefined)
  assert.equal(store.state(project, 'other').subscriptionId, 'sub-other')
  assert.equal(calls.filter((c) => c.command === 'session_unsubscribe').length, 1)
  assert.equal(calls.find((c) => c.command === 'session_unsubscribe')?.args.target.sessionId, 's')
})

test('a delayed unsubscribe cannot replace a newer session selection', async () => {
  setActivePinia(createPinia())
  const store = useChatStore()
  cleanup = () => store.$dispose()
  for (const id of ['s', 'b', 'c']) store.state(project, id).loaded = true
  let resolveRelease: () => void = () => {}
  window.__TAURI_INTERNALS__.invoke = async (command: string, args: any) => {
    if (command === 'session_unsubscribe')
      await new Promise<void>((resolve) => {
        resolveRelease = resolve
      })
    return {
      target: args.target,
      data:
        command === 'session_subscribe' ? { subscriptionId: 'sub-' + args.target.sessionId } : '',
    }
  }
  await store.select(project, 's')
  const middle = store.select(project, 'b')
  await store.select(project, 'c')
  resolveRelease()
  await middle
  assert.equal(store.active[projectKey(project)], 'c')
  assert.equal(store.state(project, 'b').subscriptionId, undefined)
  assert.equal(store.state(project, 'c').subscriptionId, 'sub-c')
})
