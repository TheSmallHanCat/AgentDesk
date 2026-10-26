import { defineStore } from 'pinia'
import { reactive, onScopeDispose } from 'vue'
import { listen } from '@tauri-apps/api/event'
import { call, desktop, errorText } from '../utils/api'
import { target, unwrap } from '../utils/target'
import { i18n } from '../i18n'
import type { Project, Reply, Target } from '../types'

export const useDeviceStore = defineStore('device', () => {
  const channels = reactive<Record<string, { state: string; enabled: boolean; error?: string }>>({})
  const key = (p: Project) => p.connectionId + ':' + p.id
  const state = (p: Project) => channels[key(p)] ?? { state: 'unprepared', enabled: false }
  const revisions = new Map<string, number>()
  let disposed = false
  const unlisten = desktop
    ? listen<{ target: Target; state: string }>('device-state', ({ payload }) => {
        for (const [id, current] of Object.entries(channels)) {
          if (!id.startsWith(payload.target.connectionId + ':')) continue
          current.state = payload.state
          if (payload.state !== 'ready') {
            current.enabled = false
            if (['closed', 'error'].includes(payload.state))
              revisions.set(id, (revisions.get(id) ?? 0) + 1)
          }
        }
      })
    : undefined
  onScopeDispose(() => {
    disposed = true
    void unlisten?.then((release) => release())
  })
  async function refresh(p: Project) {
    if (!p.deviceLocal) return
    const revision = revisions.get(key(p)) ?? 0
    const owner = target(p.connectionId, p.id, p.deviceLocal)
    const result = unwrap(
      owner,
      await call<Reply<{ state: string; enabled: boolean }>>('device_status', { target: owner }),
    )
    if (!disposed && (revisions.get(key(p)) ?? 0) === revision) channels[key(p)] = result
  }
  async function enable(p: Project) {
    if (!window.confirm(i18n.global.t('device.security', { path: p.path }))) return
    channels[key(p)] = { state: 'preparing', enabled: false }
    const revision = (revisions.get(key(p)) ?? 0) + 1
    revisions.set(key(p), revision)
    const owner = target(p.connectionId, p.id, p.deviceLocal)
    try {
      const result = unwrap(
        owner,
        await call<Reply<{ state: string; enabled: boolean }>>('device_enable', {
          target: owner,
          confirmed: true,
        }),
      )
      if (!disposed && revisions.get(key(p)) === revision) channels[key(p)] = result
    } catch (error) {
      if (!disposed && revisions.get(key(p)) === revision)
        channels[key(p)] = { state: 'error', enabled: false, error: errorText(error) }
    }
  }
  async function revoke(p: Project) {
    const revision = (revisions.get(key(p)) ?? 0) + 1
    revisions.set(key(p), revision)
    channels[key(p)] = { state: 'revoking', enabled: false }
    const owner = target(p.connectionId, p.id, p.deviceLocal)
    try {
      const result = unwrap(
        owner,
        await call<Reply<{ state: string; enabled: boolean }>>('device_revoke', { target: owner }),
      )
      if (!disposed && revisions.get(key(p)) === revision) channels[key(p)] = result
    } catch (error) {
      if (!disposed && revisions.get(key(p)) === revision)
        channels[key(p)] = { state: 'error', enabled: false, error: errorText(error) }
    }
  }
  return { channels, state, refresh, enable, revoke }
})
