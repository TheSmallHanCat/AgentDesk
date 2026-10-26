import test from 'node:test'
import assert from 'node:assert/strict'
import { createPinia, setActivePinia } from 'pinia'
import { Window } from 'happy-dom'
import type { Target } from '../src/types.ts'
Object.defineProperty(globalThis, 'window', { value: new Window(), writable: true })
Object.assign(globalThis, { isTauri: true })
Object.assign(window, {
  isTauri: true,
  __TAURI_INTERNALS__: {
    invoke: async () => null,
    transformCallback: () => 1,
    unregisterCallback: () => {},
  },
})
const { useLoginStore, agentAuthStatus } = await import('../src/stores/login.ts')
const owner: Target = { connectionId: 'a', projectId: 'p', sessionId: 's', requestSeq: 55 }
const signedOut = {
  installed: true,
  available: false,
  loggedIn: false,
  streaming: true,
  authStatus: 'loggedOut' as const,
}
function setup() {
  setActivePinia(createPinia())
  const store = useLoginStore()
  store.setCapabilities('a', { claude: { ...signedOut }, codex: { ...signedOut } })
  return store
}
test('login checks the full target, agent and login identity and clears terminal secrets', async () => {
  const store = setup()
  let channel: any
  const calls: { command: string; args: any }[] = []
  window.__TAURI_INTERNALS__.invoke = async (command: string, args: any) => {
    calls.push({ command, args })
    if (command === 'agent_login_start') {
      channel = args.onEvents
      return { target: args.target, data: { loginId: 'l', agent: args.agent } }
    }
    return { target: args.target, data: { accepted: true } }
  }
  await store.open(owner, 'claude')
  const prompt = {
    loginId: 'l',
    agent: 'claude',
    url: 'https://example.invalid/fake-secret',
    deviceCode: 'FAKE-CODE',
    needsInput: true,
  }
  channel.onmessage({ target: { ...owner, requestSeq: 56 }, method: 'login.prompt', data: prompt })
  assert.equal(store.dialog?.prompt, undefined)
  channel.onmessage({ target: owner, method: 'login.prompt', data: { ...prompt, agent: 'codex' } })
  assert.equal(store.dialog?.prompt, undefined)
  channel.onmessage({ target: owner, method: 'login.prompt', data: prompt })
  assert.equal(store.dialog?.prompt?.url, prompt.url)
  await store.respond('FAKE-INPUT')
  assert.deepEqual(calls.at(-1)?.args, { target: owner, loginId: 'l', text: 'FAKE-INPUT' })
  channel.onmessage({
    target: owner,
    method: 'login.state',
    data: { loginId: 'other', agent: 'claude', state: 'succeeded' },
  })
  assert.equal(store.dialog?.state, 'waiting')
  channel.onmessage({
    target: owner,
    method: 'login.state',
    data: {
      loginId: 'l',
      agent: 'claude',
      state: 'succeeded',
      capability: {
        ...signedOut,
        loggedIn: true,
        available: true,
        authStatus: 'loggedIn',
        accountType: 'subscription',
      },
    },
  })
  assert.equal(store.dialog?.state, 'succeeded')
  assert.equal(store.dialog?.prompt, undefined)
  assert.equal(store.capability('a', 'claude')?.accountType, 'subscription')
  channel.onmessage({ target: owner, method: 'login.prompt', data: prompt })
  assert.equal(store.dialog?.prompt, undefined)
  assert.equal(JSON.stringify(store.$state).includes('FAKE-'), false)
  await store.close()
  store.$dispose()
})
test('closing clears retained prompt objects and cancels a start that completes late', async () => {
  const store = setup()
  let resolveStart: (value: unknown) => void = () => {}
  const cancellations: any[] = []
  let channel: any
  window.__TAURI_INTERNALS__.invoke = async (command: string, args: any) => {
    if (command === 'agent_login_start') {
      channel = args.onEvents
      return new Promise((resolve) => {
        resolveStart = resolve
      })
    }
    cancellations.push(args)
    return { target: args.target, data: { accepted: true } }
  }
  const opening = store.open(owner, 'claude')
  await Promise.resolve()
  channel.onmessage({
    target: owner,
    method: 'login.prompt',
    data: { loginId: 'late', agent: 'claude', needsInput: true, rawOutput: 'FAKE-RAW' },
  })
  const retained = store.dialog!
  await store.close()
  assert.equal(retained.prompt, undefined)
  resolveStart({ target: owner, data: { loginId: 'late', agent: 'claude' } })
  await opening
  assert.equal(store.dialog, undefined)
  assert.ok(cancellations.every((args) => args.loginId === 'late' && args.target.requestSeq === 55))
  channel.onmessage({
    target: owner,
    method: 'login.prompt',
    data: { loginId: 'late', agent: 'claude', needsInput: true, rawOutput: 'FAKE-RAW' },
  })
  assert.equal(store.dialog, undefined)
  store.$dispose()
})
test('disconnection invalidates prompts and late channels without restoring a login', async () => {
  const store = setup()
  let channel: any
  window.__TAURI_INTERNALS__.invoke = async (_command: string, args: any) => {
    channel = args.onEvents
    return { target: args.target, data: { loginId: 'l', agent: args.agent } }
  }
  await store.open(owner, 'claude')
  channel.onmessage({
    target: owner,
    method: 'login.prompt',
    data: { loginId: 'l', agent: 'claude', needsInput: false, deviceCode: 'FAKE-CODE' },
  })
  store.disconnected('a')
  assert.equal(store.dialog?.state, 'disconnected')
  assert.equal(store.dialog?.prompt, undefined)
  assert.equal(store.agents.a, undefined)
  channel.onmessage({
    target: owner,
    method: 'login.state',
    data: { loginId: 'l', agent: 'claude', state: 'succeeded' },
  })
  assert.equal(store.dialog?.state, 'disconnected')
  await store.close()
  store.$dispose()
})
test('status reporting distinguishes missing, signed out, signed in and detection failure', () => {
  assert.equal(agentAuthStatus(), 'checking')
  assert.equal(agentAuthStatus({ available: false, streaming: false, installed: false }), 'missing')
  assert.equal(agentAuthStatus(signedOut), 'loggedOut')
  assert.equal(
    agentAuthStatus({ ...signedOut, loggedIn: true, authStatus: 'loggedIn' }),
    'loggedIn',
  )
  assert.equal(agentAuthStatus({ ...signedOut, authStatus: 'failed' }), 'failed')
})

test('switching targets cancels the old owner and ignores its late prompt and selection', async () => {
  const store = setup()
  const channels: any[] = []
  let resolveCancel: () => void = () => {}
  window.__TAURI_INTERNALS__.invoke = async (command: string, args: any) => {
    if (command === 'agent_login_start') {
      channels.push(args.onEvents)
      return { target: args.target, data: { loginId: 'l-' + args.agent, agent: args.agent } }
    }
    if (args.loginId === 'l-claude')
      await new Promise<void>((resolve) => {
        resolveCancel = resolve
      })
    return { target: args.target, data: { accepted: true } }
  }
  await store.open(owner, 'claude')
  const oldFrame = {
    target: owner,
    method: 'login.prompt',
    data: { loginId: 'l-claude', agent: 'claude', needsInput: true, rawOutput: 'FAKE-OLD' },
  }
  channels[0].onmessage(oldFrame)
  const retained = store.dialog!
  const obsolete = store.open({ ...owner, sessionId: 'other', requestSeq: 56 }, 'claude')
  const latestOwner = { ...owner, sessionId: 'latest', requestSeq: 57 }
  await store.open(latestOwner, 'codex')
  resolveCancel()
  await obsolete
  channels[0].onmessage(oldFrame)
  assert.equal(retained.prompt, undefined)
  assert.equal(store.dialog?.owner.sessionId, 'latest')
  assert.equal(store.dialog?.agent, 'codex')
  assert.equal(store.dialog?.prompt, undefined)
  await store.close()
  store.$dispose()
})

test('cancellation keeps the terminal event and capability when completion wins the race', async () => {
  const store = setup()
  let channel: any
  let rejectCancel: (error: unknown) => void = () => {}
  window.__TAURI_INTERNALS__.invoke = async (command: string, args: any) => {
    if (command === 'agent_login_start') {
      channel = args.onEvents
      return { target: args.target, data: { loginId: 'l', agent: args.agent } }
    }
    assert.equal(command, 'agent_login_cancel')
    assert.deepEqual(args.target, owner)
    return new Promise((_resolve, reject) => {
      rejectCancel = reject
    })
  }
  await store.open(owner, 'claude')
  const cancelling = store.cancel()
  assert.equal(store.dialog?.state, 'starting')
  assert.equal(store.dialog?.busy, true)
  channel.onmessage({
    target: owner,
    method: 'login.prompt',
    data: { loginId: 'l', agent: 'claude', needsInput: true, rawOutput: 'FAKE-LATE-PROMPT' },
  })
  assert.equal(store.dialog?.prompt, undefined)
  channel.onmessage({
    target: owner,
    method: 'login.state',
    data: {
      loginId: 'l',
      agent: 'claude',
      state: 'succeeded',
      capability: {
        ...signedOut,
        loggedIn: true,
        available: true,
        authStatus: 'loggedIn',
        accountType: 'subscription',
      },
    },
  })
  rejectCancel({ code: 'E_NOT_FOUND' })
  await cancelling
  assert.equal(store.dialog?.state, 'succeeded')
  assert.equal(store.dialog?.error, false)
  assert.equal(store.capability('a', 'claude')?.accountType, 'subscription')
  assert.equal(store.dialog?.prompt, undefined)
  await store.close()
  store.$dispose()
})

test('cancelled is shown after cleanup acknowledgement and a later terminal capability is retained', async () => {
  const store = setup()
  let channel: any
  let resolveCancel: () => void = () => {}
  window.__TAURI_INTERNALS__.invoke = async (command: string, args: any) => {
    if (command === 'agent_login_start') {
      channel = args.onEvents
      return { target: args.target, data: { loginId: 'l', agent: args.agent } }
    }
    return new Promise((resolve) => {
      resolveCancel = () => resolve({ target: args.target, data: { accepted: true } })
    })
  }
  await store.open(owner, 'claude')
  channel.onmessage({
    target: owner,
    method: 'login.prompt',
    data: { loginId: 'l', agent: 'claude', needsInput: true, rawOutput: 'FAKE-PROMPT' },
  })
  const cancelling = store.cancel()
  assert.equal(store.dialog?.state, 'waiting')
  assert.equal(store.dialog?.prompt, undefined)
  resolveCancel()
  await cancelling
  assert.equal(store.dialog?.state, 'cancelled')
  channel.onmessage({
    target: owner,
    method: 'login.state',
    data: {
      loginId: 'l',
      agent: 'claude',
      state: 'cancelled',
      capability: { ...signedOut, version: 'new-version' },
    },
  })
  assert.equal(store.capability('a', 'claude')?.version, 'new-version')
  await store.close()
  store.$dispose()
})

test('a gone login refreshes capability and ends the active state without a terminal frame', async () => {
  for (const code of ['E_NOT_FOUND', 'E_LOGIN_GONE']) {
    const store = setup()
    const commands: string[] = []
    window.__TAURI_INTERNALS__.invoke = async (command: string, args: any) => {
      commands.push(command)
      if (command === 'agent_login_start')
        return { target: args.target, data: { loginId: 'l', agent: args.agent } }
      if (command === 'agent_login_cancel') throw { code }
      assert.equal(command, 'agent_status')
      return {
        target: args.target,
        data: { claude: { ...signedOut, loggedIn: true, available: true, authStatus: 'loggedIn' } },
      }
    }
    await store.open(owner, 'claude')
    await store.cancel()
    assert.equal(commands.at(-1), 'agent_status')
    assert.equal(store.dialog?.state, 'succeeded')
    assert.equal(store.dialog?.error, false)
    assert.equal(store.dialog?.cancelRequested, false)
    assert.equal(store.capability('a', 'claude')?.loggedIn, true)
    await store.close()
    store.$dispose()
  }
})
