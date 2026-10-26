<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'
import { darkTheme, zhCN, enUS } from 'naive-ui'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import TitleBar from './components/TitleBar.vue'
import AgentLoginDialog from './components/AgentLoginDialog.vue'
import SideNav from './components/SideNav.vue'
import WorkspaceDock from './components/WorkspaceDock.vue'
import { useSettingsStore } from './stores/settings'
import { useDataStore } from './stores/data'
import { useToastStore } from './stores/toast'
import { useWorkspaceStore } from './stores/workspace'
import { useChatStore } from './stores/chat'
import { useLoginStore } from './stores/login'
import { applyTheme, themeIsDark, themeOverrides } from './utils/theme'
import { call, desktop, errorText } from './utils/api'
import { matchesShortcut } from './utils/shortcuts'
import { isRunning } from './utils/chat'
import type { ConnectionStatus, Transfer } from './types'
const { t, locale } = useI18n()
const settings = useSettingsStore()
const data = useDataStore()
const toast = useToastStore()
const workspace = useWorkspaceStore()
const chat = useChatStore()
const login = useLoginStore()
const previousConnectionStates = new Map<string, string>()
watch(
  () =>
    Object.values(data.statuses).map((value) => ({ id: value.connectionId, state: value.state })),
  (entries) => {
    for (const entry of entries) {
      const previous = previousConnectionStates.get(entry.id)
      if (entry.state !== previous) {
        if (['disconnected', 'failed', 'reconnecting'].includes(entry.state)) {
          login.disconnected(entry.id)
          if (['disconnected', 'failed'].includes(entry.state))
            void chat.releaseConnection(entry.id)
        } else if (entry.state === 'connected') {
          void login.refresh(entry.id).catch(() => {})
        }
      }
      previousConnectionStates.set(entry.id, entry.state)
    }
  },
  { flush: 'sync' },
)
const router = useRouter()
const route = useRoute()
watch(
  () => route.fullPath,
  () => void login.close(),
  { flush: 'sync' },
)
const startupError = ref('')
const closeModal = ref(false)
const closing = ref(false)
const transfers = ref<Transfer[]>([])
const ready = ref(false)
const unlisteners: UnlistenFn[] = []
const overrides = computed(() => {
  themeIsDark.value
  return themeOverrides()
})
const dirty = computed(() =>
  workspace.tabs.filter((tab) => tab.type === 'editor' && tab.content !== tab.savedContent),
)
const terminals = computed(() =>
  workspace.tabs.filter((tab) => tab.type === 'terminal' && !tab.exited),
)
const runningSessions = computed(() =>
  Object.entries(chat.states)
    .filter(([, state]) => !!state.runId && isRunning(state.state))
    .map(
      ([key]) =>
        Object.values(chat.sessions)
          .flat()
          .find((session) => key.endsWith(':' + session.id))?.title,
    )
    .filter((title): title is string => !!title),
)
watch(
  () => settings.values.theme,
  (value) => applyTheme(value),
  { immediate: true },
)
watch(
  () => settings.values.language,
  (value) => {
    locale.value = value
    document.documentElement.lang = value
  },
  { immediate: true },
)
const media = matchMedia('(prefers-color-scheme: dark)')
const systemChanged = () => {
  if (settings.values.theme === 'system') applyTheme('system')
}
media.addEventListener('change', systemChanged)
async function requestClose(forceExit = false) {
  if (!desktop) return
  if (!forceExit && settings.values.closeAction === 'tray') {
    await call('sys_hide')
    return
  }
  try {
    transfers.value = (await call<Transfer[]>('transfer_list')).filter((task) =>
      ['queued', 'running'].includes(task.state),
    )
  } catch {
    transfers.value = []
  }
  if (forceExit || settings.values.closeAction === 'exit') {
    if (
      !dirty.value.length &&
      !terminals.value.length &&
      !transfers.value.length &&
      !runningSessions.value.length
    ) {
      await exit()
      return
    }
  }
  closeModal.value = true
}
async function hide() {
  try {
    await call('sys_hide')
    closeModal.value = false
  } catch (e) {
    toast.show(errorText(e), 'error')
  }
}
async function exit() {
  if (closing.value) return
  closing.value = true
  try {
    await useChatStore().flushDrafts()
    await call('sys_exit')
  } catch (e) {
    toast.show(errorText(e), 'error')
    closing.value = false
  }
}
function shortcut(event: KeyboardEvent) {
  if (event.isComposing || event.repeat) return
  const bindings = settings.values.shortcuts
  const project = data.projects.find((p) => p.id === workspace.projectId)
  if (
    matchesShortcut(event, bindings.newTerminal) &&
    project &&
    data.online(project.connectionId)
  ) {
    event.preventDefault()
    event.stopImmediatePropagation()
    workspace.openTerminal(project)
  } else if (matchesShortcut(event, bindings.saveFile)) {
    const active = workspace.tabs.find((tab) => tab.id === workspace.panes[workspace.activePane])
    if (route.name === 'workbench' && workspace.dockOpen && active?.type === 'editor') {
      event.preventDefault()
      event.stopImmediatePropagation()
      workspace.saveRequested++
    }
  } else if (matchesShortcut(event, bindings.focusDock) && route.name === 'workbench') {
    event.preventDefault()
    event.stopImmediatePropagation()
    workspace.dockOpen = true
    workspace.focus = !workspace.focus
  } else if (route.name === 'workbench' && workspace.dockOpen) {
    if (matchesShortcut(event, bindings.closeTab)) {
      event.preventDefault()
      event.stopImmediatePropagation()
      workspace.closeRequested++
    } else if (matchesShortcut(event, bindings.nextTab)) {
      event.preventDefault()
      event.stopImmediatePropagation()
      workspace.switchTab(1)
    } else if (matchesShortcut(event, bindings.previousTab)) {
      event.preventDefault()
      event.stopImmediatePropagation()
      workspace.switchTab(-1)
    } else if (matchesShortcut(event, bindings.split)) {
      event.preventDefault()
      event.stopImmediatePropagation()
      workspace.splitRequested++
    }
  }
}
window.addEventListener('keydown', shortcut, true)
onMounted(async () => {
  try {
    await settings.load()
  } catch (e) {
    startupError.value = errorText(e)
  }
  try {
    if (desktop) {
      await call('term_reset')
      unlisteners.push(
        await listen<ConnectionStatus>('connection-state', (event) =>
          data.applyState(event.payload),
        ),
      )
      unlisteners.push(
        await listen<{ forceExit: boolean }>(
          'app-close-requested',
          (event) => void requestClose(event.payload.forceExit),
        ),
      )
      unlisteners.push(
        await listen<{ projectId?: string; sessionId?: string }>('notification-open', (event) => {
          if (
            event.payload.projectId &&
            data.projects.some((p) => p.id === event.payload.projectId)
          )
            void router.push(
              '/workbench/' +
                event.payload.projectId +
                (event.payload.sessionId ? '/' + encodeURIComponent(event.payload.sessionId) : ''),
            )
        }),
      )
      unlisteners.push(
        await listen<{ connectionId: string }>('connection-lost', (event) => {
          void call('sys_notify', {
            kind: 'disconnected',
            title: t('notifications.lostTitle'),
            body: t('notifications.lostBody'),
            projectId:
              data.projects.find((p) => p.connectionId === event.payload.connectionId)?.id ?? null,
          }).catch((e) => toast.show(errorText(e), 'error'))
        }),
      )
      await data.refresh()
      const last = localStorage.getItem('agentdesk-last-project')
      if (
        last &&
        data.projects.some((p) => p.id === last && !p.archived) &&
        ['/', '/connections'].includes(route.path)
      )
        await router.replace('/workbench/' + last)
    }
  } catch (e) {
    startupError.value = errorText(e)
  } finally {
    ready.value = true
  }
})
onUnmounted(() => {
  for (const unlisten of unlisteners) unlisten()
  window.removeEventListener('keydown', shortcut, true)
  media.removeEventListener('change', systemChanged)
})
</script>
<template>
  <n-config-provider
    :locale="settings.values.language === 'zh-CN' ? zhCN : enUS"
    :theme="themeIsDark ? darkTheme : null"
    :theme-overrides="overrides"
    ><n-dialog-provider
      ><div class="app-shell">
        <TitleBar @close="requestClose()" /><SideNav />
        <main class="main-area">
          <n-alert v-if="startupError" type="error" closable @close="startupError = ''">{{
            startupError
          }}</n-alert>
          <div class="route-area">
            <router-view v-if="ready" />
            <div v-else class="empty">{{ t('common.loading') }}</div>
          </div>
          <WorkspaceDock v-if="ready" />
        </main>
      </div>
      <div v-if="toast.text" class="toast">
        <n-alert :type="toast.kind" closable @close="toast.text = ''">{{ toast.text }}</n-alert>
      </div>
      <AgentLoginDialog />
      <n-modal v-model:show="closeModal" :mask-closable="!closing"
        ><n-card style="width: 600px" :title="t('close.title')" closable @close="closeModal = false"
          ><p>{{ t('close.choose') }}</p>
          <n-alert v-if="dirty.length || terminals.length || transfers.length" type="warning">{{
            t('close.warning')
          }}</n-alert>
          <p v-else-if="!runningSessions.length" class="muted">{{ t('close.noResources') }}</p>
          <template v-if="runningSessions.length">
            <h3>{{ t('close.runs') }}</h3>
            <p>{{ t('close.runsContinue') }}</p>
            <ul>
              <li v-for="(title, index) in runningSessions" :key="index">{{ title }}</li>
            </ul>
          </template>
          <template v-if="dirty.length"
            ><h3>{{ t('close.unsaved') }}</h3>
            <ul>
              <li v-for="tab in dirty" :key="tab.id" class="mono">
                {{ tab.target.connectionId }} · {{ tab.path }}
              </li>
            </ul></template
          ><template v-if="terminals.length"
            ><h3>{{ t('close.terminals') }}</h3>
            <ul>
              <li v-for="tab in terminals" :key="tab.id" class="mono">
                {{ tab.title }} · {{ tab.path }}
              </li>
            </ul></template
          ><template v-if="transfers.length"
            ><h3>{{ t('tasks.title') }}</h3>
            <ul>
              <li v-for="task in transfers" :key="task.id" class="mono">{{ task.path }}</li>
            </ul></template
          ><template #footer
            ><div class="actions" style="justify-content: flex-end">
              <n-button :disabled="closing" @click="closeModal = false">{{
                t('common.cancel')
              }}</n-button
              ><n-button :disabled="closing" @click="hide">{{ t('close.tray') }}</n-button
              ><n-button type="error" :loading="closing" @click="exit">{{
                t('close.exit')
              }}</n-button>
            </div></template
          ></n-card
        ></n-modal
      ></n-dialog-provider
    ></n-config-provider
  >
</template>
