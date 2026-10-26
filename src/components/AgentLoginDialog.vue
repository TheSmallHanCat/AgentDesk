<script setup lang="ts">
import { computed, onUnmounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { openUrl } from '@tauri-apps/plugin-opener'
import { useLoginStore } from '../stores/login'
const { t, te } = useI18n()
const login = useLoginStore()
const text = ref('')
const logoutConfirm = ref(false)
const copied = ref('')
const actionError = ref(false)
let copyTimer: ReturnType<typeof setTimeout> | undefined
const current = computed(() => login.dialog)
const active = computed(() => ['starting', 'waiting'].includes(current.value?.state ?? ''))
const reason = computed(() => {
  if (!current.value?.reason) return ''
  const key = 'login.reason.' + current.value.reason
  return t(te(key) ? key : 'login.reason.UNKNOWN')
})
const url = computed(() => {
  try {
    const parsed = new URL(current.value?.prompt?.url ?? '')
    return ['https:', 'http:'].includes(parsed.protocol) && !parsed.username && !parsed.password
      ? parsed.href
      : ''
  } catch {
    return ''
  }
})
watch(
  () => [current.value, current.value?.state],
  () => {
    text.value = ''
    logoutConfirm.value = false
    copied.value = ''
    actionError.value = false
    clearTimeout(copyTimer)
  },
  { flush: 'sync' },
)
onUnmounted(() => {
  clearTimeout(copyTimer)
  text.value = ''
  void login.close()
})
async function copy(value: string, kind: string) {
  try {
    await navigator.clipboard.writeText(value)
    copied.value = kind
    clearTimeout(copyTimer)
    copyTimer = setTimeout(() => {
      copied.value = ''
    }, 1800)
  } catch {
    actionError.value = true
  }
}
async function browse() {
  if (!url.value) return
  try {
    await openUrl(url.value)
  } catch {
    actionError.value = true
  }
}
async function submit() {
  const value = text.value
  text.value = ''
  await login.respond(value)
}
function close() {
  text.value = ''
  void login.close()
}
</script>
<template>
  <n-modal :show="!!current" :mask-closable="true" @update:show="(value) => !value && close()">
    <n-card
      v-if="current"
      class="agent-login-dialog login-dialog"
      role="dialog"
      aria-modal="true"
      :aria-label="t('login.title', { agent: current.agent })"
      :title="t('login.title', { agent: current.agent })"
      closable
      @close="close"
    >
      <p class="login-state" role="status" aria-live="polite">
        {{ t('login.state.' + current.state) }}
      </p>
      <p v-if="current.state === 'loggedIn'" class="muted">
        {{ t('login.accountType') }} ·
        {{
          login.capability(current.owner.connectionId, current.agent)?.accountType ??
          t('login.accountUnknown')
        }}
      </p>
      <p v-if="active" class="muted">{{ t('login.instructions') }}</p>
      <section v-if="url" class="login-link">
        <p class="mono login-url">{{ url }}</p>
        <div class="actions">
          <n-button type="primary" data-testid="login-open-browser" @click="browse">{{
            t('login.openBrowser')
          }}</n-button
          ><n-button @click="copy(url, 'url')">{{
            copied === 'url' ? t('login.copied') : t('login.copyLink')
          }}</n-button>
        </div>
      </section>
      <section v-if="current.prompt?.deviceCode" class="login-device-code">
        <strong>{{ current.prompt.deviceCode }}</strong>
        <p>{{ t('login.deviceInstruction') }}</p>
        <n-button @click="copy(current.prompt!.deviceCode!, 'code')">{{
          copied === 'code' ? t('login.copied') : t('login.copyCode')
        }}</n-button>
      </section>
      <section v-if="current.prompt?.rawOutput" class="login-fallback">
        <p class="muted">{{ t('login.fallback') }}</p>
        <pre>{{ current.prompt.rawOutput }}</pre>
      </section>
      <n-form
        v-if="active && (current.prompt?.needsInput || current.prompt?.rawOutput)"
        @submit.prevent="submit"
      >
        <n-form-item :label="t('login.inputLabel')"
          ><n-input
            v-model:value="text"
            type="password"
            show-password-on="click"
            :input-props="{
              autocomplete: 'off',
              spellcheck: false,
              'aria-label': t('login.inputLabel'),
            }"
            :disabled="current.busy"
            @keydown.enter.prevent="submit"
        /></n-form-item>
        <n-button type="primary" :loading="current.busy" :disabled="!text.trim()" @click="submit">{{
          t('login.submit')
        }}</n-button>
      </n-form>
      <n-alert v-if="reason" :type="current.state === 'succeeded' ? 'success' : 'warning'">{{
        reason
      }}</n-alert>
      <n-alert v-if="current.error || actionError" type="error">{{
        t('login.actionFailed')
      }}</n-alert>
      <n-alert v-if="logoutConfirm" type="warning"
        ><p>{{ t('login.logoutConfirm') }}</p>
        <div class="actions">
          <n-button :disabled="current.busy" @click="logoutConfirm = false">{{
            t('common.cancel')
          }}</n-button
          ><n-button type="error" :loading="current.busy" @click="login.logout()">{{
            t('login.confirmLogout')
          }}</n-button>
        </div></n-alert
      >
      <template #footer
        ><div class="actions login-footer">
          <n-button v-if="active" :disabled="current.busy" @click="login.cancel()">{{
            t('common.cancel')
          }}</n-button>
          <n-button v-else @click="close">{{ t('common.close') }}</n-button>
          <n-button
            v-if="current.state === 'loggedIn' && !logoutConfirm"
            :disabled="current.busy"
            @click="logoutConfirm = true"
            >{{ t('login.logout') }}</n-button
          >
          <n-button
            v-if="
              ['loggedOut', 'failed', 'expired', 'cancelled', 'disconnected'].includes(
                current.state,
              )
            "
            type="primary"
            :disabled="current.state === 'disconnected'"
            :loading="current.busy"
            @click="login.start()"
            >{{ t('login.retry') }}</n-button
          >
        </div></template
      >
    </n-card>
  </n-modal>
</template>
<style scoped>
.agent-login-dialog {
  width: min(560px, calc(100vw - 32px));
  max-height: calc(100vh - 40px);
  overflow: auto;
}
.login-state {
  font-weight: 600;
}
.login-url {
  overflow-wrap: anywhere;
  font-size: 12px;
  line-height: 1.7;
}
.login-device-code {
  margin: 24px 0;
  padding: 20px;
  text-align: center;
  background: var(--surface2);
  border: 1px solid var(--border);
  border-radius: 8px;
}
.login-device-code strong {
  font-family: var(--font-mono);
  font-size: clamp(24px, 5vw, 34px);
  letter-spacing: 0.12em;
  overflow-wrap: anywhere;
}
.login-device-code p,
.muted {
  font-size: 12px;
  line-height: 1.7;
}
.login-fallback pre {
  max-height: 240px;
  overflow: auto;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  background: var(--surface2);
  padding: 14px;
  font-size: 12px;
}
.login-footer {
  justify-content: flex-end;
  flex-wrap: wrap;
}
.n-alert {
  margin-top: 16px;
}
</style>
