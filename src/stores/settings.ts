import { defineStore } from 'pinia'
import { reactive } from 'vue'
import type { Settings } from '../types.ts'
import { call, desktop } from '../utils/api.ts'
export const defaultSettings = (): Settings => ({
  theme: 'system',
  language: 'zh-CN',
  closeAction: 'ask',
  scrollback: 10000,
  fontSize: 14,
  notifications: { disconnected: true, completed: true, approval: true },
  shortcuts: {
    newTerminal: 'Ctrl+Shift+T',
    saveFile: 'Ctrl+S',
    focusDock: 'Ctrl+Shift+F',
    closeTab: 'Ctrl+Shift+W',
    nextTab: 'Ctrl+PageDown',
    previousTab: 'Ctrl+PageUp',
    split: 'Ctrl+Alt+S',
  },
})
export const useSettingsStore = defineStore('settings', () => {
  const values = reactive(defaultSettings())
  async function load() {
    const saved = desktop
      ? await call<Settings>('settings_get')
      : (JSON.parse(
          localStorage.getItem('agentdesk-settings') ?? 'null',
        ) as Partial<Settings> | null)
    if (saved)
      Object.assign(values, defaultSettings(), saved, {
        notifications: { ...defaultSettings().notifications, ...saved.notifications },
        shortcuts: { ...defaultSettings().shortcuts, ...saved.shortcuts },
      })
  }
  async function save() {
    if (desktop) await call('settings_set', { settings: JSON.parse(JSON.stringify(values)) })
    else localStorage.setItem('agentdesk-settings', JSON.stringify(values))
  }
  return { values, load, save }
})
