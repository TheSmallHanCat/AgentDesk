import test from 'node:test'
import assert from 'node:assert/strict'
import { createPinia, setActivePinia } from 'pinia'
import { Window } from 'happy-dom'
Object.defineProperty(globalThis, 'window', { value: new Window(), writable: true })
Object.assign(globalThis, { isTauri: true })
Object.assign(window, {
  isTauri: true,
  __TAURI_INTERNALS__: {
    invoke: async () => {
      throw new Error('No IPC fixture installed')
    },
  },
})
const { useWorkspaceStore } = await import('../src/stores/workspace.ts')
test('splits are limited to four and closing a split preserves tabs', () => {
  setActivePinia(createPinia())
  const store = useWorkspaceStore()
  const project = {
    id: 'p',
    connectionId: 'a',
    path: '/app',
    name: 'App',
    description: '',
    defaultAgent: 'claude' as const,
    defaultModel: '',
    permissionMode: 'default',
    pinned: false,
    archived: false,
  }
  for (let i = 0; i < 6; i++) store.openTerminal(project)
  for (let i = 0; i < 8; i++) store.split()
  assert.equal(store.panes.length, 4)
  const tabs = store.tabs.length
  store.unsplit()
  assert.equal(store.panes.length, 3)
  assert.equal(store.tabs.length, tabs)
})
test('opening a terminal on another project does not mutate existing targets', () => {
  setActivePinia(createPinia())
  const store = useWorkspaceStore()
  const project = {
    id: 'p',
    connectionId: 'a',
    path: '/app',
    name: 'App',
    description: '',
    defaultAgent: 'claude' as const,
    defaultModel: '',
    permissionMode: 'default',
    pinned: false,
    archived: false,
  }
  store.openTerminal(project)
  const original = JSON.stringify(store.tabs[0].target)
  store.openTerminal({ ...project, id: 'other', connectionId: 'b', path: '/other' })
  assert.equal(JSON.stringify(store.tabs[0].target), original)
  assert.equal(store.tabs[1].target.connectionId, 'b')
})

const pendingProject = {
  id: 'pending',
  connectionId: 'a',
  path: '/app',
  name: 'App',
  description: '',
  defaultAgent: 'claude' as const,
  defaultModel: '',
  permissionMode: 'default',
  pinned: false,
  archived: false,
}
function installReads() {
  const pending: {
    target: import('../src/types.ts').Target
    resolve: (value: unknown) => void
    path: string
  }[] = []
  Object.assign(window, {
    __TAURI_INTERNALS__: {
      invoke: (
        _command: string,
        args: { target: import('../src/types.ts').Target; path: string },
      ) => new Promise((resolve) => pending.push({ ...args, resolve })),
    },
  })
  return pending
}
function finishRead(read: ReturnType<typeof installReads>[number]) {
  read.resolve({
    target: read.target,
    data: {
      path: read.path,
      content: 'text',
      mtime: 1,
      size: 4,
      permissions: 33188,
      lineEnding: 'lf',
    },
  })
}
test('late file response is discarded after switching away and back', async () => {
  setActivePinia(createPinia())
  const store = useWorkspaceStore()
  store.projectId = pendingProject.id
  const pending = installReads()
  const opening = store.openFile(pendingProject, '/app/old.txt')
  store.projectId = 'other'
  store.projectId = pendingProject.id
  finishRead(pending[0])
  await opening
  assert.equal(store.tabs.length, 0)
})
test('a later file request prevents an earlier response from selecting a tab', async () => {
  setActivePinia(createPinia())
  const store = useWorkspaceStore()
  store.projectId = pendingProject.id
  const pending = installReads()
  const old = store.openFile(pendingProject, '/app/old.txt')
  const latest = store.openFile(pendingProject, '/app/latest.txt')
  finishRead(pending[1])
  await latest
  finishRead(pending[0])
  await old
  assert.equal(store.tabs.length, 1)
  assert.equal(store.tabs[0].path, '/app/latest.txt')
})

test('native terminal tabs preserve an agent identifier supplied by capabilities', () => {
  setActivePinia(createPinia())
  const store = useWorkspaceStore()
  store.openTerminal(pendingProject, 'registered-agent')
  assert.equal(store.tabs[0].kind, 'registered-agent')
  assert.match(store.tabs[0].title, /^registered-agent · /)
})
