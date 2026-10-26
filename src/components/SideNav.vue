<script setup lang="ts">
import { useI18n } from 'vue-i18n'
import { DesktopOutline, ServerOutline, FolderOpenOutline, SwapVerticalOutline, ConstructOutline, PulseOutline, SettingsOutline } from '@vicons/ionicons5'
const { t } = useI18n()
const items = [{ name: 'workbench', icon: DesktopOutline }, { name: 'connections', icon: ServerOutline }, { name: 'projects', icon: FolderOpenOutline }, { name: 'tasks', icon: SwapVerticalOutline }, { name: 'environment', icon: ConstructOutline }, { name: 'diagnostics', icon: PulseOutline }, { name: 'settings', icon: SettingsOutline }]
</script>
<template><nav class="side-nav" :aria-label="t('nav.label')"><router-link v-for="item in items" :key="item.name" :to="'/' + item.name" :class="{ bottom: item.name === 'diagnostics' }" :title="t('nav.' + item.name)"><component :is="item.icon" /><span>{{ t('nav.' + item.name) }}</span></router-link></nav></template>
