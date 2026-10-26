<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useDeviceStore } from '../stores/device'
import { useChatStore } from '../stores/chat'
import { agentAuthStatus } from '../stores/login'
import AgentLoginStatus from './AgentLoginStatus.vue'
import { errorText } from '../utils/api'
import type { Project } from '../types'
const props = defineProps<{ project?: Project }>()
const emit = defineEmits<{ close: [] }>()
const { t } = useI18n(),
  chat = useChatStore()
const device = useDeviceStore()
const agent = ref(''),
  model = ref(''),
  mode = ref('default'),
  busy = ref(false),
  error = ref('')
const capabilities = computed(() =>
  props.project ? (chat.agents[props.project.connectionId] ?? {}) : {},
)
const selected = computed(() => capabilities.value[agent.value])
const options = computed(() =>
  Object.entries(capabilities.value).map(([id, caps]) => ({
    value: id,
    label:
      id +
      ' · ' +
      t('login.auth.' + agentAuthStatus(caps)) +
      (caps.loggedIn && caps.accountType ? ' · ' + caps.accountType : ''),
    disabled: !caps.installed,
  })),
)
const models = computed(() =>
  (selected.value?.models ?? []).map((row) => ({
    value: String(row.id),
    label: String(row.name ?? row.id),
  })),
)
const modes = computed(() =>
  (selected.value?.permissionModes ?? []).map((value) => ({
    value,
    label: t('chat.mode.' + value, value),
    disabled: !!props.project?.deviceLocal && value === 'plan',
  })),
)
watch(
  () => props.project,
  (project) => {
    agent.value =
      Object.entries(project ? (chat.agents[project.connectionId] ?? {}) : {}).find(
        ([, caps]) => caps.available && caps.loggedIn,
      )?.[0] ??
      Object.keys(project ? (chat.agents[project.connectionId] ?? {}) : {})[0] ??
      ''
    error.value = ''
  },
  { immediate: true },
)
watch(
  selected,
  () => {
    model.value = models.value[0]?.value ?? ''
    mode.value = modes.value[0]?.value ?? 'default'
  },
  { immediate: true },
)
async function create() {
  if (!props.project) return
  busy.value = true
  error.value = ''
  try {
    await chat.create(props.project, agent.value, model.value, mode.value)
    emit('close')
  } catch (value) {
    error.value = errorText(value)
  } finally {
    busy.value = false
  }
}
</script>
<template>
  <n-modal
    :show="!!project"
    :mask-closable="!busy"
    @update:show="(value) => !value && emit('close')"
    ><n-card class="session-dialog" :title="t('chat.newSession')" closable @close="emit('close')"
      ><n-form label-placement="top"
        ><n-form-item :label="t('chat.agent')"
          ><n-select v-model:value="agent" :options="options" :placeholder="t('chat.chooseAgent')"
        /></n-form-item>
        <p
          v-if="!Object.values(capabilities).some((value) => value.available && value.loggedIn)"
          class="muted"
        >
          {{ t('chat.noReadyAgent') }}
          <router-link to="/environment" @click="emit('close')">{{
            t('environment.title')
          }}</router-link>
        </p>
        <AgentLoginStatus
          v-if="project && agent"
          :agent="agent"
          :connection-id="project.connectionId"
          :project-id="project.id"
          :device-local="project.deviceLocal"
          :capability="selected"
          :disabled="busy"
        />
        <p v-if="selected?.reason" class="muted">{{ selected.reason }}</p>
        <n-form-item :label="t('chat.model')"
          ><n-select
            v-model:value="model"
            :options="models"
            :disabled="!selected?.available" /></n-form-item
        ><n-form-item :label="t('chat.permission')"
          ><n-select v-model:value="mode" :options="modes"
        /></n-form-item>
        <p v-if="project?.deviceLocal" class="muted">{{ t('device.planUnavailable') }}</p>
        <n-alert v-if="error" type="error">{{ error }}</n-alert
        ><n-button
          type="primary"
          :loading="busy"
          :disabled="
            !agent ||
            !model ||
            !selected?.available ||
            !selected?.loggedIn ||
            (!!project?.deviceLocal && mode === 'plan') ||
            (!!project?.deviceLocal && !device.state(project).enabled)
          "
          @click="create"
          >{{ t('chat.create') }}</n-button
        ></n-form
      ></n-card
    ></n-modal
  >
</template>
<style scoped>
.session-dialog {
  width: min(480px, calc(100vw - 32px));
}
</style>
