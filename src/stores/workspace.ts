import { defineStore } from 'pinia'
import { i18n } from '../i18n/index.ts'
import { ref, watch } from 'vue'
import type { Target, FileDocument, Reply, Project } from '../types.ts'
import { target, unwrap, sameResource, RequestGuard } from '../utils/target.ts'
import { call } from '../utils/api.ts'
interface BaseTab {
  id: string
  target: Target
  title: string
  path: string
  error: string
}
export interface TerminalTab extends BaseTab {
  type: 'terminal'
  kind: 'shell' | 'claude' | 'codex'
  terminalId?: string
  exited: boolean
}
export interface EditorTab extends BaseTab {
  type: 'editor'
  document: FileDocument
  content: string
  savedContent: string
  revision: number
}
export type WorkTab = TerminalTab | EditorTab
export const useWorkspaceStore = defineStore('workspace', () => {
  const tabs = ref<WorkTab[]>([])
  const panes = ref<string[]>([])
  const activePane = ref(0)
  const dockOpen = ref(false)
  const focus = ref(false)
  const saveRequested = ref(0)
  const projectId = ref('')
  const ratioX = ref(50)
  const ratioY = ref(50)
  const dockHeight = ref(320)
  const closeRequested = ref(0)
  const splitRequested = ref(0)
  const reads = new RequestGuard()
  watch(projectId, () => reads.invalidate(), { flush: 'sync' })
  function switchTab(direction: number) {
    const index = tabs.value.findIndex((t) => t.id === panes.value[activePane.value])
    if (tabs.value.length)
      select(tabs.value[(index + direction + tabs.value.length) % tabs.value.length].id)
  }
  function select(id: string) {
    const other = panes.value.indexOf(id)
    if (other >= 0) activePane.value = other
    else if (panes.value.length) panes.value[activePane.value] = id
    else {
      panes.value = [id]
      activePane.value = 0
    }
    dockOpen.value = true
  }
  function openTerminal(
    project: Project,
    kind: TerminalTab['kind'] = 'shell',
    path = project.path,
  ) {
    const id = crypto.randomUUID()
    tabs.value.push({
      id,
      type: 'terminal',
      target: target(project.connectionId, project.id, project.deviceLocal),
      title:
        kind === 'shell'
          ? 'Shell'
          : (kind === 'claude' ? 'Claude Code' : 'Codex') +
            ' · ' +
            i18n.global.t('workbench.independent'),
      path,
      error: '',
      kind,
      exited: false,
    })
    select(id)
  }
  async function openFile(project: Project, path: string) {
    const request = reads.begin(project.connectionId, project.id, project.deviceLocal)
    const existing = tabs.value.find(
      (t) => t.type === 'editor' && t.path === path && sameResource(t.target, request),
    )
    if (existing) {
      select(existing.id)
      return
    }
    const reply = await call<Reply<FileDocument>>('fs_read', { target: request, path })
    if (!reads.accepts(reply.target) || projectId.value !== project.id) return
    const document = unwrap(reads.current!, reply)
    const raced = tabs.value.find(
      (t) => t.type === 'editor' && t.path === path && sameResource(t.target, request),
    )
    if (raced) {
      select(raced.id)
      return
    }
    const id = crypto.randomUUID()
    tabs.value.push({
      id,
      type: 'editor',
      target: request,
      title: path.split('/').pop() ?? path,
      path,
      error: '',
      document,
      content: document.content,
      savedContent: document.content,
      revision: 0,
    })
    select(id)
  }
  async function closeTab(id: string) {
    const tab = tabs.value.find((t) => t.id === id)
    if (!tab) return
    if (tab.type === 'terminal' && tab.terminalId)
      await call('term_close', { target: tab.target, terminalId: tab.terminalId })
    tabs.value = tabs.value.filter((t) => t.id !== id)
    const replacement = tabs.value.at(-1)?.id
    panes.value = panes.value
      .map((p) => (p === id ? (replacement ?? '') : p))
      .filter((p, i, a) => p && a.indexOf(p) === i)
    activePane.value = Math.max(0, Math.min(activePane.value, panes.value.length - 1))
  }
  function split() {
    if (panes.value.length >= 4) return
    const candidate = tabs.value.find((t) => !panes.value.includes(t.id))
    if (candidate) {
      panes.value.push(candidate.id)
      activePane.value = panes.value.length - 1
      focus.value = true
      dockOpen.value = true
    }
  }
  function unsplit() {
    if (panes.value.length > 1) {
      panes.value.splice(activePane.value, 1)
      activePane.value = Math.max(0, activePane.value - 1)
    }
  }
  return {
    tabs,
    panes,
    activePane,
    dockOpen,
    focus,
    saveRequested,
    projectId,
    ratioX,
    ratioY,
    dockHeight,
    closeRequested,
    splitRequested,
    select,
    openTerminal,
    openFile,
    closeTab,
    split,
    unsplit,
    switchTab,
  }
})
