import { defineStore } from 'pinia'
import { reactive, ref } from 'vue'
import { Channel } from '@tauri-apps/api/core'
import { call } from '../utils/api.ts'
import { sameTarget, target, unwrap } from '../utils/target.ts'
import type { Reply, Target } from '../types.ts'
import type { AgentCapability, LoginPrompt, LoginState } from '../protocol/generated.ts'

export type AuthStatus = NonNullable<AgentCapability['authStatus']>
export function agentAuthStatus(capability?: AgentCapability): AuthStatus {
  if (capability?.authStatus) return capability.authStatus
  if (!capability) return 'checking'
  if (!capability.installed) return 'missing'
  return capability.loggedIn ? 'loggedIn' : 'loggedOut'
}
export interface LoginFrame {
  target: Target
  method: 'login.prompt' | 'login.state'
  data: LoginPrompt | LoginState
  disconnected?: boolean
}
export interface LoginDialog {
  owner: Target
  agent: string
  loginId?: string
  state: LoginState['state'] | AuthStatus | 'disconnected'
  prompt?: LoginPrompt
  reason?: string
  error: boolean
  busy: boolean
  cancelRequested?: boolean
}
const running = (state: LoginDialog['state']) => ['starting', 'waiting'].includes(state)

export const useLoginStore = defineStore('login', () => {
  const agents = reactive<Record<string, Record<string, AgentCapability>>>({})
  const dialog = ref<LoginDialog>()
  let revision = 0
  const probes = new Map<string, number>()
  function setCapabilities(connectionId: string, capabilities: Record<string, AgentCapability>) {
    probes.set(connectionId, (probes.get(connectionId) ?? 0) + 1)
    agents[connectionId] = capabilities
  }
  function capability(connectionId: string, agent: string) {
    return agents[connectionId]?.[agent]
  }
  function updateCapability(owner: Target, agent: string, value?: AgentCapability) {
    if (value) {
      probes.set(owner.connectionId, (probes.get(owner.connectionId) ?? 0) + 1)
      ;(agents[owner.connectionId] ??= {})[agent] = value
    }
  }
  async function refresh(connectionId: string) {
    const request = (probes.get(connectionId) ?? 0) + 1
    probes.set(connectionId, request)
    const owner = target(connectionId)
    const values = unwrap(
      owner,
      await call<Reply<Record<string, AgentCapability>>>('agent_status', { target: owner }),
    )
    if (probes.get(connectionId) === request) setCapabilities(connectionId, values)
  }
  async function cancelSession(owner: Target, loginId: string) {
    unwrap(
      owner,
      await call<Reply<{ accepted: boolean }>>('agent_login_cancel', { target: owner, loginId }),
    )
  }
  async function close() {
    const previous = dialog.value
    const shouldCancel = previous && running(previous.state)
    ++revision
    if (previous) {
      previous.prompt = undefined
      previous.reason = undefined
    }
    dialog.value = undefined
    if (previous?.loginId && shouldCancel)
      await cancelSession(previous.owner, previous.loginId).catch(() => {})
  }
  async function start() {
    const current = dialog.value
    if (!current || current.busy || running(current.state)) return
    const request = ++revision
    const owner = { ...current.owner }
    current.state = 'starting'
    current.prompt = undefined
    current.reason = undefined
    current.error = false
    current.busy = true
    current.cancelRequested = false
    current.loginId = undefined
    const channel = new Channel<LoginFrame>()
    channel.onmessage = (frame) => {
      if (revision !== request || dialog.value !== current || !sameTarget(owner, frame.target))
        return
      if (frame.disconnected) {
        current.prompt = undefined
        current.reason = undefined
        current.state = 'disconnected'
        current.busy = false
        ++revision
        return
      }
      const incoming = frame.data
      if (
        incoming.agent !== current.agent ||
        (current.loginId && incoming.loginId !== current.loginId)
      )
        return
      current.loginId = incoming.loginId
      if (frame.method === 'login.prompt') {
        if (!running(current.state) || current.cancelRequested) return
        current.prompt = { ...(incoming as LoginPrompt) }
        current.state = 'waiting'
      } else {
        const state = incoming as LoginState
        if (!running(current.state) && !(current.state === 'cancelled' && !running(state.state)))
          return
        current.state = state.state
        current.reason = state.reason
        updateCapability(owner, current.agent, state.capability)
        if (!running(current.state)) {
          current.prompt = undefined
          current.busy = false
        }
      }
    }
    try {
      const result = unwrap(
        owner,
        await call<Reply<{ loginId: string; agent: string }>>('agent_login_start', {
          target: owner,
          agent: current.agent,
          onEvents: channel,
        }),
      )
      if (revision !== request || dialog.value !== current) {
        await cancelSession(owner, result.loginId).catch(() => {})
        return
      }
      if (result.agent !== current.agent || (current.loginId && current.loginId !== result.loginId))
        throw new Error('Login ownership mismatch')
      current.loginId = result.loginId
    } catch {
      if (revision === request && dialog.value === current) {
        current.prompt = undefined
        current.state = 'failed'
        current.error = true
      }
    } finally {
      if (revision === request && dialog.value === current) current.busy = false
    }
  }
  async function open(owner: Target, agent: string) {
    const closing = close()
    const opening = revision
    await closing
    if (revision !== opening) return
    dialog.value = {
      owner: { ...owner },
      agent,
      state: agentAuthStatus(capability(owner.connectionId, agent)),
      error: false,
      busy: false,
    }
    if (!['loggedIn', 'missing', 'checking'].includes(dialog.value.state)) await start()
  }
  async function respond(text: string) {
    const current = dialog.value,
      request = revision
    if (!current?.loginId || current.busy || !running(current.state) || !text.trim()) return
    current.busy = true
    current.error = false
    try {
      unwrap(
        current.owner,
        await call<Reply<{ accepted: boolean }>>('agent_login_respond', {
          target: current.owner,
          loginId: current.loginId,
          text,
        }),
      )
    } catch {
      if (revision === request && dialog.value === current) {
        current.error = true
      }
    } finally {
      if (revision === request && dialog.value === current) current.busy = false
    }
  }
  async function cancel() {
    const current = dialog.value
    if (!current?.loginId || current.cancelRequested || !running(current.state)) return
    const request = revision
    current.cancelRequested = true
    current.prompt = undefined
    current.busy = true
    current.error = false
    try {
      await cancelSession(current.owner, current.loginId)
      if (revision === request && dialog.value === current && running(current.state))
        current.state = 'cancelled'
    } catch (error) {
      if (revision === request && dialog.value === current && running(current.state)) {
        const code = typeof error === 'object' && error && 'code' in error ? error.code : ''
        if (code === 'E_NOT_FOUND' || code === 'E_LOGIN_GONE') {
          try {
            await refresh(current.owner.connectionId)
          } catch {
            if (revision === request && dialog.value === current) current.error = true
          }
          if (revision === request && dialog.value === current && running(current.state))
            current.state = capability(current.owner.connectionId, current.agent)?.loggedIn
              ? 'succeeded'
              : 'cancelled'
        } else current.error = true
      }
    } finally {
      if (revision === request && dialog.value === current) {
        current.cancelRequested = false
        current.busy = false
      }
    }
  }
  async function logout() {
    const current = dialog.value,
      request = revision
    if (!current || current.busy || current.state !== 'loggedIn') return
    current.busy = true
    current.error = false
    try {
      const result = unwrap(
        current.owner,
        await call<Reply<{ loggedOut: boolean; capability?: AgentCapability }>>('agent_logout', {
          target: current.owner,
          agent: current.agent,
        }),
      )
      if (revision !== request || dialog.value !== current) return
      updateCapability(current.owner, current.agent, result.capability)
      if (!result.capability) await refresh(current.owner.connectionId)
      if (revision === request && dialog.value === current) current.state = 'loggedOut'
    } catch {
      if (revision === request && dialog.value === current) current.error = true
    } finally {
      if (revision === request && dialog.value === current) current.busy = false
    }
  }
  function disconnected(connectionId: string) {
    probes.set(connectionId, (probes.get(connectionId) ?? 0) + 1)
    delete agents[connectionId]
    if (dialog.value?.owner.connectionId !== connectionId) return
    ++revision
    dialog.value.prompt = undefined
    dialog.value.reason = undefined
    dialog.value.state = 'disconnected'
    dialog.value.busy = false
  }
  return {
    agents,
    dialog,
    capability,
    setCapabilities,
    refresh,
    open,
    start,
    respond,
    cancel,
    close,
    logout,
    disconnected,
  }
})
