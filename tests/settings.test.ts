import test from 'node:test'
import assert from 'node:assert/strict'
import { Window } from 'happy-dom'
import { createPinia, setActivePinia } from 'pinia'
import { matchesShortcut } from '../src/utils/shortcuts.ts'
const browser = new Window({ url: 'http://localhost' })
Object.defineProperty(globalThis, 'window', { value: browser, writable: true })
Object.defineProperty(globalThis, 'localStorage', { value: browser.localStorage, writable: true })
const { useSettingsStore, defaultSettings } = await import('../src/stores/settings.ts')
test('browser settings save and load without native services', async () => {
  setActivePinia(createPinia())
  const store = useSettingsStore()
  assert.equal(store.values.language, 'zh-CN')
  assert.equal(store.values.scrollback, 10000)
  store.values.theme = 'dark'
  store.values.language = 'en'
  await store.save()
  setActivePinia(createPinia())
  const second = useSettingsStore()
  await second.load()
  assert.equal(second.values.theme, 'dark')
  assert.equal(second.values.language, 'en')
  assert.deepEqual(Object.keys(second.values.notifications).sort(), [
    'approval',
    'completed',
    'disconnected',
  ])
})
test('terminal control keys are not captured by default global shortcuts', () => {
  const binding = defaultSettings().shortcuts
  for (const key of ['c', 'r', 'a']) {
    const event = { key, ctrlKey: true, altKey: false, shiftKey: false, metaKey: false }
    for (const shortcut of Object.values(binding))
      assert.equal(matchesShortcut(event, shortcut), false)
  }
  assert.equal(
    matchesShortcut(
      { key: 'T', ctrlKey: true, altKey: false, shiftKey: true, metaKey: false },
      binding.newTerminal,
    ),
    true,
  )
})
