<script setup lang="ts">
import { useRoute } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { desktop } from '../utils/api'
const emit = defineEmits<{ close: [] }>()
const route = useRoute()
const { t } = useI18n()
function minimize() {
  if (desktop) void getCurrentWindow().minimize()
}
function maximize() {
  if (desktop) void getCurrentWindow().toggleMaximize()
}
function drag(e: MouseEvent) {
  if (e.detail === 2) maximize()
  else if (desktop) void getCurrentWindow().startDragging()
}
</script>
<template>
  <header class="title-bar">
    <div class="brand" @mousedown.left="drag">
      <b class="brand-mark">A</b><strong>AgentDesk</strong
      ><span class="muted">/ {{ t('nav.' + String(route.name ?? 'connections')) }}</span>
    </div>
    <div class="title-drag" @mousedown.left="drag" />
    <div class="window-actions">
      <button :aria-label="t('common.minimize')" @click="minimize">−</button
      ><button :aria-label="t('common.maximize')" @click="maximize">□</button
      ><button class="close" :aria-label="t('common.close')" @click="emit('close')">×</button>
    </div>
  </header>
</template>
