<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { agentAuthStatus, useLoginStore } from '../stores/login'
import { target } from '../utils/target'
import type { AgentCapability } from '../protocol/generated'
const props = defineProps<{
  agent: string
  connectionId: string
  projectId?: string
  sessionId?: string
  deviceLocal?: boolean
  capability?: AgentCapability
  disabled?: boolean
}>()
const { t } = useI18n()
const login = useLoginStore()
const capability = computed(
  () => login.capability(props.connectionId, props.agent) ?? props.capability,
)
const state = computed(() => agentAuthStatus(capability.value))
function open() {
  void login.open(
    {
      ...target(props.connectionId, props.projectId, props.deviceLocal),
      ...(props.sessionId ? { sessionId: props.sessionId } : {}),
    },
    props.agent,
  )
}
</script>
<template>
  <span class="agent-login-status">
    <span role="status"
      >{{ t('login.auth.' + state)
      }}<template v-if="state === 'loggedIn' && capability?.accountType">
        · {{ capability.accountType }}</template
      ></span
    >
    <n-button
      v-if="agent !== 'fake' && !['missing', 'checking'].includes(state)"
      size="small"
      :disabled="disabled"
      @click="open"
      >{{ state === 'loggedIn' ? t('login.manage') : t('login.login') }}</n-button
    >
  </span>
</template>
<style scoped>
.agent-login-status {
  display: inline-flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
  font-size: 12px;
}
</style>
