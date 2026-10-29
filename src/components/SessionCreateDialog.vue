<script setup lang="ts">
import { computed, onUnmounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useDeviceStore } from '../stores/device'
import { useChatStore } from '../stores/chat'
import { agentAuthStatus } from '../stores/login'
import AgentLoginStatus from './AgentLoginStatus.vue'
import { errorText } from '../utils/api'
import type { Project } from '../types'
const props = defineProps<{ project?: Project }>()
const emit = defineEmits<{ close: []; created: [project: Project, sessionId: string] }>()
const { t } = useI18n(),
  chat = useChatStore()
const device = useDeviceStore()
let alive = true
onUnmounted(() => {
  alive = false
})
const agent = ref(''),
  model = ref(''),
  mode = ref('default'),
  busy = ref(false),
  error = ref('')
const capabilities = computed(() => (props.project ? chat.capabilities(props.project) : {}))
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
  [() => props.project, capabilities],
  ([project, values], [previous]) => {
    if (project !== previous || !values[agent.value]?.installed) {
      const preferred = project?.defaultAgent ?? ''
      agent.value =
        (values[preferred]?.available && values[preferred]?.loggedIn ? preferred : undefined) ??
        Object.keys(values).find((id) => values[id].available && values[id].loggedIn) ??
        (values[preferred]?.installed ? preferred : undefined) ??
        Object.keys(values).find((id) => values[id].installed) ??
        ''
    }
    if (project !== previous) error.value = ''
  },
  { immediate: true },
)
watch(
  [() => props.project, agent, selected],
  ([project, agentId], [previousProject, previousAgent]) => {
    const changed = project !== previousProject || agentId !== previousAgent
    if (changed || !models.value.some((value) => value.value === model.value)) {
      const preferred = agentId === project?.defaultAgent ? project.defaultModel : ''
      model.value =
        models.value.find((value) => value.value === preferred)?.value ??
        models.value[0]?.value ??
        ''
    }
    const allowed = modes.value.filter((value) => !value.disabled)
    if (changed || !allowed.some((value) => value.value === mode.value))
      mode.value =
        allowed.find((value) => value.value === project?.permissionMode)?.value ??
        allowed.find((value) => value.value === 'default')?.value ??
        allowed[0]?.value ??
        ''
  },
  { immediate: true },
)
async function create() {
  if (!props.project || busy.value) return
  busy.value = true
  error.value = ''
  const project = props.project
  try {
    const session = await chat.create(project, agent.value, model.value, mode.value)
    if (!alive || props.project !== project) return
    emit('created', project, session.id)
    emit('close')
  } catch (value) {
    if (alive && props.project === project) error.value = errorText(value)
  } finally {
    busy.value = false
  }
}
</script>
<template>
  <n-modal
    :show="!!project"
    :mask-closable="!busy"
    :close-on-esc="!busy"
    @update:show="(value) => !value && emit('close')"
    ><n-card
      class="session-dialog"
      :title="t('chat.newSession')"
      :closable="!busy"
      @close="emit('close')"
      ><n-form label-placement="top"
        ><n-form-item :label="t('chat.agent')"
          ><n-select v-model:value="agent" :options="options" :placeholder="t('chat.chooseAgent')"
        /></n-form-item>
        <p
          v-if="!Object.values(capabilities).some((value) => value.available && value.loggedIn)"
          class="muted"
        >
          {{ t('chat.noReadyAgent') }}
          <router-link
            :to="{
              name: 'environment',
              params: { connectionId: project?.connectionId },
              query: { projectId: project?.id },
            }"
            @click="emit('close')"
            >{{ t('environment.title') }}</router-link
          >
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
            !mode ||
            !selected?.available ||
            !selected?.loggedIn ||
            (!!project?.deviceLocal && mode === 'plan') ||
            (!!project?.deviceLocal &&
              (!device.state(project).enabled || device.state(project).state !== 'ready'))
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
