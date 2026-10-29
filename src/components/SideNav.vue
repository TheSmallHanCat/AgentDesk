<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute } from 'vue-router'
import { useDataStore } from '../stores/data'
import { useWorkspaceStore } from '../stores/workspace'
import { useChatStore } from '../stores/chat'
import {
  DesktopOutline,
  ServerOutline,
  FolderOpenOutline,
  SwapVerticalOutline,
  ConstructOutline,
  PulseOutline,
  SettingsOutline,
} from '@vicons/ionicons5'
const { t } = useI18n()
const route = useRoute()
const data = useDataStore()
const workspace = useWorkspaceStore()
const chat = useChatStore()
const project = computed(() =>
  data.projects.find((value) => value.id === workspace.projectId && !value.archived),
)
function destination(name: string) {
  if (name === 'workbench' && project.value)
    return {
      name,
      params: { projectId: project.value.id, sessionId: chat.selected(project.value)?.id },
    }
  if (name === 'environment') {
    if (route.name === name) return route.fullPath
    if (project.value)
      return {
        name,
        params: { connectionId: project.value.connectionId },
        query: { projectId: project.value.id },
      }
  }
  return { name }
}
const items = [
  { name: 'workbench', icon: DesktopOutline },
  { name: 'connections', icon: ServerOutline },
  { name: 'projects', icon: FolderOpenOutline },
  { name: 'tasks', icon: SwapVerticalOutline },
  { name: 'environment', icon: ConstructOutline },
  { name: 'diagnostics', icon: PulseOutline },
  { name: 'settings', icon: SettingsOutline },
]
</script>
<template>
  <nav class="side-nav" :aria-label="t('nav.label')">
    <router-link
      v-for="item in items"
      :key="item.name"
      :to="destination(item.name)"
      :class="{ bottom: item.name === 'diagnostics' }"
      :title="t('nav.' + item.name)"
      ><component :is="item.icon" /><span>{{ t('nav.' + item.name) }}</span></router-link
    >
  </nav>
</template>
