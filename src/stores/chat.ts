import { defineStore } from 'pinia'
import { reactive, watch, onScopeDispose } from 'vue'
import { Channel } from '@tauri-apps/api/core'
import { i18n } from '../i18n/index.ts'
import { call, errorText } from '../utils/api.ts'
import { target, unwrap, sameTarget } from '../utils/target.ts'
import { timeline, applyEvents, type ChatTimeline, type Delivery } from '../utils/chat.ts'
import { useSettingsStore } from './settings.ts'
import { useLoginStore } from './login.ts'
import type { Project, Reply, Target } from '../types.ts'
import type {
  Session,
  HostEvent,
  AgentCapability,
  ApprovalDecision,
  HistoryResult,
} from '../protocol/generated.ts'

export interface PendingMessage {
  id: string
  text: string
  status: Delivery
}
export interface SessionState extends ChatTimeline {
  draft: string
  draftLoaded: boolean
  error: string
  loading: boolean
  loaded: boolean
  hasMore: boolean
  nextBeforeSeq?: number
  pending: PendingMessage[]
  subscriptionId?: string
  subscriptionTarget?: Target
  subscriptionRevision?: number
  subscribing?: boolean
}
export const projectKey = (project: Pick<Project, 'connectionId' | 'id'>) =>
  project.connectionId + ':' + project.id
export const sessionKey = (project: Pick<Project, 'connectionId' | 'id'>, id: string) =>
  projectKey(project) + ':' + id
const requestTarget = (project: Project, sessionId?: string): Target => ({
  ...target(project.connectionId, project.id, project.deviceLocal),
  ...(sessionId ? { sessionId } : {}),
})

export const useChatStore = defineStore('chat', () => {
  const sessions = reactive<Record<string, Session[]>>({})
  const active = reactive<Record<string, string>>({})
  const states = reactive<Record<string, SessionState>>({})
  const agents = useLoginStore().agents
  const errors = reactive<Record<string, string>>({})
  const loading = reactive<Record<string, boolean>>({})
  const archived = reactive<Record<string, boolean>>({})
  const revisions = new Map<string, number>()
  const stopVersions = new Map<string, number>()
  let disposed = false
  const draftVersions = new Map<string, number>()
  const draftTimers = new Map<string, ReturnType<typeof setTimeout>>()
  const draftPending = new Map<string, { project: Project; sessionId: string; text: string }>()
  const draftWrites = new Map<string, Promise<void>>()
  function queueDraft(key: string, entry: { project: Project; sessionId: string; text: string }) {
    const previous = draftWrites.get(key) ?? Promise.resolve()
    const next = previous
      .catch(() => {})
      .then(async () => {
        if (disposed || !states[key]) return
        const owner = requestTarget(entry.project, entry.sessionId)
        unwrap(
          owner,
          await call<Reply<null>>('session_draft_set', { target: owner, text: entry.text }),
        )
      })
    draftWrites.set(key, next)
    void next.catch((error) => {
      if (states[key]) states[key].error = errorText(error)
    })
    return next
  }
  async function flushDrafts() {
    for (const [key, entry] of draftPending) {
      clearTimeout(draftTimers.get(key))
      draftTimers.delete(key)
      draftPending.delete(key)
      queueDraft(key, entry)
    }
    await Promise.all([...draftWrites.values()])
  }
  function state(project: Project, sessionId: string): SessionState {
    const key = sessionKey(project, sessionId)
    if (!states[key]) {
      states[key] = {
        ...timeline(),
        draft: '',
        draftLoaded: false,
        error: '',
        loading: false,
        loaded: false,
        hasMore: false,
        pending: [],
      }
      const current = states[key]
      watch(
        () => current.draft,
        (text) => {
          draftVersions.set(key, (draftVersions.get(key) ?? 0) + 1)
          clearTimeout(draftTimers.get(key))
          draftPending.set(key, { project, sessionId, text })
          draftTimers.set(
            key,
            setTimeout(() => {
              const entry = draftPending.get(key)
              draftPending.delete(key)
              draftTimers.delete(key)
              if (entry) queueDraft(key, entry)
            }, 300),
          )
        },
        { flush: 'sync' },
      )
    }
    return states[key]
  }
  async function loadDraft(project: Project, sessionId: string) {
    const current = state(project, sessionId),
      key = sessionKey(project, sessionId)
    if (current.draftLoaded) return
    current.draftLoaded = true
    const revision = draftVersions.get(key) ?? 0,
      owner = requestTarget(project, sessionId)
    try {
      const text = unwrap(owner, await call<Reply<string>>('session_draft_get', { target: owner }))
      if (states[key] && (draftVersions.get(key) ?? 0) === revision) current.draft = text
    } catch (error) {
      current.draftLoaded = false
      current.error = errorText(error)
    }
  }
  onScopeDispose(() => {
    disposed = true
    for (const timer of draftTimers.values()) clearTimeout(timer)
    draftPending.clear()
    draftTimers.clear()
  })
  function list(project: Project) {
    return sessions[projectKey(project)] ?? []
  }
  function selected(project: Project) {
    return list(project).find((value) => value.id === active[projectKey(project)])
  }
  async function refresh(project: Project) {
    const key = projectKey(project)
    const revision = (revisions.get(key) ?? 0) + 1
    revisions.set(key, revision)
    loading[key] = true
    errors[key] = ''
    try {
      const owner = requestTarget(project)
      const rows = unwrap(
        owner,
        await call<Reply<Session[]>>('session_list', {
          target: owner,
          archived: archived[key] ?? false,
        }),
      )
      if (revisions.get(key) !== revision) return
      sessions[key] = rows
      const probe = requestTarget(project)
      const environment = unwrap(
        probe,
        await call<Reply<{ agents: Record<string, AgentCapability> }>>('env_probe', {
          target: probe,
        }),
      )
      if (revisions.get(key) !== revision) return
      useLoginStore().setCapabilities(project.connectionId, environment.agents)
      const nextId = rows.some((value) => value.id === active[key]) ? active[key] : rows[0]?.id
      if (nextId) await select(project, nextId)
      else {
        const previous = active[key]
        active[key] = ''
        if (previous) await releaseSession(state(project, previous))
      }
    } catch (error) {
      if (revisions.get(key) === revision) errors[key] = errorText(error)
    } finally {
      if (revisions.get(key) === revision) loading[key] = false
    }
  }
  function receive(
    project: Project,
    sessionId: string,
    events: HostEvent[],
    notifications: boolean,
  ) {
    const current = state(project, sessionId)
    const incoming = events.filter((event) => event.sessionId === sessionId)
    const batchSeen = new Set<number>()
    const fresh = incoming.filter((event) => {
      if (current.seen.has(event.seq) || batchSeen.has(event.seq)) return false
      batchSeen.add(event.seq)
      return true
    })
    applyEvents(current, incoming)
    for (const event of fresh) {
      if (event.type === 'run.state' && event.payload.state === 'stopping') {
        const key = sessionKey(project, sessionId)
        stopVersions.set(key, (stopVersions.get(key) ?? 0) + 1)
      }
      if (event.type === 'user.message') {
        const message = current.pending.find((value) => value.id === event.payload.clientMsgId)
        if (message) message.status = 'delivered'
      }
      if (event.type === 'item.completed' && event.payload.delivery === 'undelivered') {
        const message = current.pending.find((value) => value.id === event.payload.clientMsgId)
        if (message) message.status = 'undelivered'
      }
      if (event.type === 'session.updated') {
        const session = list(project).find((value) => value.id === sessionId)
        if (session && typeof event.payload.title === 'string') session.title = event.payload.title
      }
      if (notifications && (event.type === 'approval.request' || event.type === 'run.finished')) {
        const kind = event.type === 'approval.request' ? 'approval' : 'completed'
        if (useSettingsStore().values.notifications[kind])
          void call('sys_notify', {
            kind,
            title: i18n.global.t('chat.notify.' + kind),
            body: list(project).find((value) => value.id === sessionId)?.title ?? project.name,
            projectId: project.id,
            sessionId,
          }).catch(() => {})
      }
    }
    const session = list(project).find((value) => value.id === sessionId)
    if (session && fresh.length) {
      session.status = current.runId ? current.state : 'idle'
      session.updatedAt = fresh.at(-1)!.ts
    }
  }
  async function releaseSession(current: SessionState) {
    const id = current.subscriptionId,
      owner = current.subscriptionTarget
    current.subscriptionRevision = (current.subscriptionRevision ?? 0) + 1
    current.subscriptionId = undefined
    current.subscriptionTarget = undefined
    current.subscribing = false
    if (id && owner) await call('session_unsubscribe', { target: owner, subscriptionId: id })
  }
  async function restore(project: Project, sessionId: string) {
    const owner = requestTarget(project, sessionId)
    unwrap(owner, await call<Reply<Session>>('session_restore', { target: owner }))
    await refresh(project)
  }
  async function select(project: Project, sessionId: string) {
    const previous = active[projectKey(project)]
    active[projectKey(project)] = sessionId
    if (previous && previous !== sessionId) await releaseSession(state(project, previous))
    if (active[projectKey(project)] !== sessionId) return
    const current = state(project, sessionId)
    void loadDraft(project, sessionId)
    if (!current.loaded) await history(project, sessionId)
    if (active[projectKey(project)] !== sessionId) return
    if (current.subscriptionId || current.subscribing) return
    current.subscribing = true
    const subscriptionRevision = current.subscriptionRevision ?? 0
    const owner = requestTarget(project, sessionId)
    const channel = new Channel<{
      target: Target
      events: HostEvent[]
      disconnected?: boolean
      closed?: boolean
    }>()
    channel.onmessage = (batch) => {
      if (
        !sameTarget(owner, batch.target) ||
        (current.subscriptionRevision ?? 0) !== subscriptionRevision
      )
        return
      receive(project, sessionId, batch.events, true)
      if (!batch.disconnected && current.error === i18n.global.t('chat.disconnected'))
        current.error = ''
      if (batch.closed) {
        current.subscriptionId = undefined
        current.subscriptionTarget = undefined
        current.subscriptionRevision = subscriptionRevision + 1
        current.subscribing = false
      }
      if (batch.disconnected) {
        current.pending
          .filter((value) => value.status === 'sending')
          .forEach((value) => {
            value.status = 'unknown'
          })
        current.error = i18n.global.t('chat.disconnected')
      }
    }
    try {
      const result = unwrap(
        owner,
        await call<Reply<{ subscriptionId: string }>>('session_subscribe', {
          target: owner,
          lastSeq: current.lastSeq,
          onEvents: channel,
        }),
      )
      if ((current.subscriptionRevision ?? 0) !== subscriptionRevision) {
        await call('session_unsubscribe', {
          target: owner,
          subscriptionId: result.subscriptionId,
        }).catch(() => {})
        return
      }
      current.subscriptionId = result.subscriptionId
      current.subscriptionTarget = owner
      if (current.error === i18n.global.t('chat.disconnected')) current.error = ''
    } catch (error) {
      current.error = errorText(error)
    } finally {
      if ((current.subscriptionRevision ?? 0) === subscriptionRevision) current.subscribing = false
    }
  }
  async function releaseConnection(connectionId: string) {
    const cleanup: Promise<unknown>[] = []
    for (const [key, current] of Object.entries(states)) {
      if (!key.startsWith(connectionId + ':')) continue
      const id = current.subscriptionId,
        owner = current.subscriptionTarget
      current.subscriptionRevision = (current.subscriptionRevision ?? 0) + 1
      current.subscriptionId = undefined
      current.subscriptionTarget = undefined
      current.subscribing = false
      current.pending
        .filter((message) => message.status === 'sending')
        .forEach((message) => {
          message.status = 'unknown'
        })
      if (id && owner)
        cleanup.push(call('session_unsubscribe', { target: owner, subscriptionId: id }))
    }
    await Promise.allSettled(cleanup)
  }
  async function history(project: Project, sessionId: string, more = false) {
    const current = state(project, sessionId)
    if (current.loading) return
    current.loading = true
    const owner = requestTarget(project, sessionId)
    try {
      const result = unwrap(
        owner,
        await call<Reply<HistoryResult>>('session_history', {
          target: owner,
          ...(more && current.nextBeforeSeq !== undefined
            ? { beforeSeq: current.nextBeforeSeq }
            : {}),
          limit: 50,
        }),
      )
      receive(project, sessionId, result.events, false)
      current.hasMore = result.hasMore
      current.nextBeforeSeq = result.nextBeforeSeq
      current.loaded = true
    } catch (error) {
      current.error = errorText(error)
    } finally {
      current.loading = false
    }
  }
  async function create(project: Project, agent: string, model: string, permissionMode: string) {
    const owner = requestTarget(project)
    const created = unwrap(
      owner,
      await call<Reply<Session>>('session_create', {
        target: owner,
        agent,
        model,
        permissionMode,
        title: i18n.global.t('chat.newSession'),
      }),
    )
    ;(sessions[projectKey(project)] ??= []).unshift(created)
    await select(project, created.id)
    return created
  }
  async function rename(project: Project, sessionId: string, title: string) {
    const owner = requestTarget(project, sessionId)
    const updated = unwrap(
      owner,
      await call<Reply<Session>>('session_rename', { target: owner, title }),
    )
    sessions[projectKey(project)] = list(project).map((value) =>
      value.id === sessionId ? updated : value,
    )
  }
  async function remove(project: Project, sessionId: string) {
    const owner = requestTarget(project, sessionId)
    unwrap(owner, await call<Reply<unknown>>('session_delete', { target: owner }))
    const current = state(project, sessionId)
    if (current.subscriptionId)
      await call('session_unsubscribe', {
        target: current.subscriptionTarget,
        subscriptionId: current.subscriptionId,
      })
    const draftKey = sessionKey(project, sessionId)
    clearTimeout(draftTimers.get(draftKey))
    draftTimers.delete(draftKey)
    draftPending.delete(draftKey)
    stopVersions.delete(draftKey)
    delete states[draftKey]
    sessions[projectKey(project)] = list(project).filter((value) => value.id !== sessionId)
    if (active[projectKey(project)] === sessionId)
      active[projectKey(project)] = list(project)[0]?.id ?? ''
    if (active[projectKey(project)]) await select(project, active[projectKey(project)])
  }
  async function send(project: Project, sessionId: string) {
    const current = state(project, sessionId)
    const text = current.draft.trim()
    if (
      !text ||
      current.state === 'stopping' ||
      current.pending.some((value) => value.status === 'sending')
    )
      return
    const message = reactive<PendingMessage>({ id: crypto.randomUUID(), text, status: 'sending' })
    current.pending.push(message)
    current.draft = ''
    current.error = ''
    const owner = requestTarget(project, sessionId)
    const key = sessionKey(project, sessionId)
    const stopVersion = stopVersions.get(key) ?? 0
    const runId = current.runId
    let command = runId ? 'run_append' : 'run_send'
    let retrySend = false
    try {
      let result: { runId?: string } = {}
      for (let attempt = 0; ; attempt++) {
        if (message.status !== 'sending')
          throw { code: 'E_DISCONNECTED', message: 'Input delivery was interrupted' }
        if (
          disposed ||
          states[key] !== current ||
          (stopVersions.get(key) ?? 0) !== stopVersion ||
          current.state === 'stopping' ||
          (active[projectKey(project)] && active[projectKey(project)] !== sessionId)
        )
          throw { code: 'E_APPEND_CLOSED', message: 'Input delivery was cancelled' }
        try {
          result = unwrap(
            owner,
            await call<Reply<{ runId?: string }>>(command, {
              target: owner,
              text,
              clientMsgId: message.id,
              ...(command === 'run_append' ? { runId } : {}),
            }),
          )
          break
        } catch (error) {
          const code =
            typeof error === 'object' && error && 'code' in error ? String(error.code) : ''
          if (attempt >= 20) throw error
          if (command === 'run_append' && code === 'E_APPEND_CLOSED') {
            command = 'run_send'
            retrySend = true
            message.id = crypto.randomUUID()
          } else if (
            !(command === 'run_append' && code === 'E_APPEND_NOT_READY') &&
            !(retrySend && code === 'E_SESSION_BUSY')
          )
            throw error
          await new Promise((resolve) => setTimeout(resolve, 150))
        }
      }
      const delivered = current.pending.find((value) => value.id === message.id)
      if (delivered && delivered.status !== 'undelivered') delivered.status = 'delivered'
      if (!current.runId && !['completed', 'interrupted', 'failed'].includes(current.state))
        current.runId = result.runId
    } catch (error) {
      const code = typeof error === 'object' && error && 'code' in error ? String(error.code) : ''
      message.status = ['E_DELIVERY_UNKNOWN', 'E_DISCONNECTED', 'E_TIMEOUT', 'E_IO'].includes(code)
        ? 'unknown'
        : 'failed'
      current.error = errorText(error)
    }
  }
  async function stop(project: Project, sessionId: string) {
    const current = state(project, sessionId)
    if (!current.runId) return
    const key = sessionKey(project, sessionId)
    stopVersions.set(key, (stopVersions.get(key) ?? 0) + 1)
    current.state = 'stopping'
    const owner = requestTarget(project, sessionId)
    try {
      unwrap(owner, await call<Reply<unknown>>('run_stop', { target: owner, runId: current.runId }))
    } catch (error) {
      current.error = errorText(error)
    }
  }
  async function approve(
    project: Project,
    sessionId: string,
    approvalId: string,
    decision: ApprovalDecision,
  ) {
    const owner = requestTarget(project, sessionId)
    try {
      unwrap(
        owner,
        await call<Reply<unknown>>('approval_respond', { target: owner, approvalId, decision }),
      )
    } catch (error) {
      if (
        typeof error === 'object' &&
        error &&
        'code' in error &&
        error.code === 'E_ALREADY_RESOLVED' &&
        'details' in error &&
        error.details &&
        typeof error.details === 'object' &&
        'decision' in error.details
      ) {
        const item = state(project, sessionId).items.find(
          (item) => item.id === 'approval:' + approvalId,
        )
        if (item) {
          item.payload.decision = error.details.decision
          item.complete = true
        }
      }
      throw error
    }
  }
  return {
    archived,
    restore,
    sessions,
    active,
    states,
    agents,
    errors,
    loading,
    list,
    selected,
    state,
    refresh,
    select,
    history,
    create,
    rename,
    remove,
    send,
    stop,
    approve,
    flushDrafts,
    releaseConnection,
  }
})
