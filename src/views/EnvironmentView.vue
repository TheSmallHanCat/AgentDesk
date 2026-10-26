<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import { Channel } from '@tauri-apps/api/core'
import { useI18n } from 'vue-i18n'
import { useDataStore } from '../stores/data'
import { useWorkspaceStore } from '../stores/workspace'
import { call, errorText } from '../utils/api'
import { target, unwrap, sameTarget } from '../utils/target'
import type { Reply, Target } from '../types'
import type { AgentCapability } from '../protocol/generated'
const { t } = useI18n(),
  data = useDataStore(),
  workspace = useWorkspaceStore()
interface Environment {
  layers: { name: string; status: string; detail: string }[]
  agents: Record<string, AgentCapability>
  host: {
    installed: boolean
    running: boolean
    version?: string
    hostId?: string
    activeRuns?: number
  }
  sameHostConnections?: string[]
}
interface Plan {
  planId: string
  artifacts: Record<string, unknown>[]
  paths: string[]
  commands: string
  [key: string]: unknown
}
const route = useRoute()
const rollbackConfirm = ref(false)
const connectionId = ref(
    typeof route.params.connectionId === 'string' ? route.params.connectionId : '',
  ),
  environment = ref<Environment>(),
  busy = ref(false),
  error = ref(''),
  plan = ref<Plan>(),
  running = ref(false),
  logs = ref(''),
  fakeAgent = ref(false)
let revision = 0
let installationOwner: Target | undefined
const options = computed(() =>
  data.connections.map((connection) => ({
    label: connection.name,
    value: connection.id,
    disabled: !data.online(connection.id),
  })),
)
const online = computed(() => data.online(connectionId.value))
watch(
  () => data.connections.map((value) => value.id).join(','),
  () => {
    if (!connectionId.value)
      connectionId.value = data.connections.find((value) => data.online(value.id))?.id ?? ''
  },
  { immediate: true },
)
watch(
  connectionId,
  () => {
    environment.value = undefined
    plan.value = undefined
    error.value = ''
    void probe()
  },
  { immediate: true },
)
async function probe() {
  if (!online.value) return
  const request = ++revision,
    owner = target(connectionId.value)
  busy.value = true
  error.value = ''
  try {
    const result = unwrap(owner, await call<Reply<Environment>>('env_probe', { target: owner }))
    if (request === revision) environment.value = result
  } catch (value) {
    if (request === revision) error.value = errorText(value)
  } finally {
    if (request === revision) busy.value = false
  }
}
async function prepare() {
  const request = revision,
    owner = target(connectionId.value)
  busy.value = true
  error.value = ''
  try {
    const result = unwrap(
      owner,
      await call<Reply<Plan>>('env_install_plan', { target: owner, fakeAgent: fakeAgent.value }),
    )
    if (request === revision) plan.value = result
  } catch (value) {
    if (request === revision) error.value = errorText(value)
  } finally {
    if (request === revision) busy.value = false
  }
}
async function install() {
  if (!plan.value) return
  const saved = plan.value,
    owner = target(connectionId.value),
    request = revision
  installationOwner = owner
  running.value = true
  logs.value = ''
  const progress = new Channel<{
    target: Target
    message?: string
    done: boolean
    success?: boolean
  }>()
  progress.onmessage = (frame) => {
    if (!sameTarget(owner, frame.target)) return
    if (frame.message) logs.value = (logs.value + frame.message + '\n').slice(-100000)
  }
  try {
    unwrap(
      owner,
      await call<Reply<unknown>>('env_install_run', {
        target: owner,
        planId: saved.planId,
        onProgress: progress,
      }),
    )
    if (request === revision) {
      plan.value = undefined
      await probe()
    }
  } catch (value) {
    if (request === revision) error.value = errorText(value)
  } finally {
    running.value = false
    installationOwner = undefined
  }
}
async function cancel() {
  if (!installationOwner || !plan.value) return
  try {
    unwrap(
      installationOwner,
      await call<Reply<unknown>>('env_install_cancel', {
        target: installationOwner,
        planId: plan.value.planId,
      }),
    )
  } catch (value) {
    error.value = errorText(value)
  }
}
async function hostAction(command: 'host_start' | 'host_stop' | 'host_rollback') {
  const request = revision,
    owner = target(connectionId.value)
  busy.value = true
  error.value = ''
  try {
    unwrap(
      owner,
      await call<Reply<unknown>>(command, {
        target: owner,
        ...(command !== 'host_stop' ? { fakeAgent: fakeAgent.value } : {}),
      }),
    )
    if (request === revision) await probe()
  } catch (value) {
    if (request === revision) error.value = errorText(value)
  } finally {
    if (request === revision) busy.value = false
  }
}
function login(agent: string) {
  workspace.openLoginTerminal(connectionId.value, agent)
}
</script>
<template>
  <section class="page environment-page">
    <div class="page-heading">
      <div>
        <h1>{{ t('environment.title') }}</h1>
        <p class="muted">{{ t('environment.description') }}</p>
      </div>
      <n-select
        v-model:value="connectionId"
        :options="options"
        :disabled="running"
        :placeholder="t('environment.chooseConnection')"
        style="width: 240px"
      />
    </div>
    <n-alert v-if="error" type="error" closable @close="error = ''">{{ error }}</n-alert>
    <div v-if="!online" class="empty">
      <h2>{{ t('environment.connectFirst') }}</h2>
      <router-link to="/connections">{{ t('nav.connections') }}</router-link>
    </div>
    <template v-else>
      <div class="environment-tools">
        <n-button :loading="busy" :disabled="running" @click="probe">{{
          t('environment.probe')
        }}</n-button
        ><n-button :disabled="busy || running || !!environment?.host.activeRuns" @click="prepare">{{
          t('environment.installPlan')
        }}</n-button
        ><n-checkbox v-model:checked="fakeAgent" :disabled="running">{{
          t('environment.fakeAgent')
        }}</n-checkbox>
      </div>
      <div v-if="environment" class="environment-grid">
        <section class="probe-panel">
          <h2>{{ t('environment.preflight') }}</h2>
          <div v-for="layer in environment.layers" :key="layer.name" class="probe-row">
            <div>
              <strong>{{ t('environment.layer.' + layer.name, layer.name) }}</strong>
              <p>{{ layer.detail }}</p>
            </div>
            <n-tag
              size="small"
              :type="
                layer.status === 'ready'
                  ? 'success'
                  : layer.status === 'failed'
                    ? 'error'
                    : 'warning'
              "
              >{{ t('environment.status.' + layer.status, layer.status) }}</n-tag
            >
          </div>
        </section>
        <section class="host-panel">
          <h2>Agent Host</h2>
          <p>
            {{ environment.host.running ? t('environment.running') : t('environment.stopped') }} ·
            {{ environment.host.version ?? '—' }}
          </p>
          <p v-if="environment.host.hostId" class="mono muted">{{ environment.host.hostId }}</p>
          <p v-if="environment.sameHostConnections?.length" class="host-warning">
            {{
              t('environment.sameHost', {
                names: environment.sameHostConnections
                  .map(
                    (id) => data.connections.find((connection) => connection.id === id)?.name ?? id,
                  )
                  .join(', '),
              })
            }}
          </p>
          <div class="actions">
            <n-button
              :disabled="busy || running || environment.host.running || !environment.host.installed"
              @click="hostAction('host_start')"
              >{{ t('environment.startHost') }}</n-button
            ><n-button
              :disabled="
                busy || running || !environment.host.running || !!environment.host.activeRuns
              "
              @click="hostAction('host_stop')"
              >{{ t('environment.stopHost') }}</n-button
            >
            <n-button
              :disabled="
                busy || running || !environment.host.installed || !!environment.host.activeRuns
              "
              @click="rollbackConfirm = true"
              >{{ t('environment.rollback') }}</n-button
            >
          </div>
          <p v-if="environment.host.activeRuns" class="muted">
            {{ t('environment.activeRuns', { count: environment.host.activeRuns }) }}
          </p>
        </section>
      </div>
      <section v-if="environment" class="agents-panel">
        <h2>{{ t('environment.agents') }}</h2>
        <article v-for="(caps, id) in environment.agents" :key="id" class="agent-row">
          <div>
            <strong>{{ id }}</strong>
            <p class="muted">
              {{ caps.version ?? '—' }} ·
              {{
                caps.loggedIn
                  ? t('chat.agentReady')
                  : caps.installed
                    ? t('chat.agentLoginRequired')
                    : t('chat.agentMissing')
              }}
            </p>
            <small v-if="caps.accountType">{{ caps.accountType }} · </small
            ><small v-if="caps.reason">{{ caps.reason }}</small>
          </div>
          <n-button
            v-if="id !== 'fake'"
            :disabled="!caps.installed || running"
            @click="login(String(id))"
            >{{ t('environment.login') }}</n-button
          >
        </article>
        <p class="muted">{{ t('environment.loginHelp') }}</p>
      </section>
      <section v-if="logs" class="install-log">
        <h2>{{ t('environment.installLog') }}</h2>
        <pre aria-live="polite">{{ logs }}</pre>
      </section>
    </template>
    <n-modal :show="rollbackConfirm" @update:show="(value) => (rollbackConfirm = value)"
      ><n-card style="width: 480px" :title="t('environment.rollback')"
        ><p>{{ t('environment.rollbackConfirm') }}</p>
        <div class="actions">
          <n-button @click="rollbackConfirm = false">{{ t('common.cancel') }}</n-button
          ><n-button
            type="warning"
            @click="
              () => {
                rollbackConfirm = false
                hostAction('host_rollback')
              }
            "
            >{{ t('environment.confirmRollback') }}</n-button
          >
        </div></n-card
      ></n-modal
    >
    <n-modal
      :show="!!plan"
      :mask-closable="!running"
      @update:show="(value) => !value && !running && (plan = undefined)"
      ><n-card class="install-plan" :title="t('environment.installPlan')"
        ><p>{{ t('environment.planConfirm') }}</p>
        <h3>{{ t('environment.artifacts') }}</h3>
        <pre>{{ JSON.stringify(plan?.artifacts, null, 2) }}</pre>
        <h3>{{ t('environment.paths') }}</h3>
        <pre>{{ plan?.paths.join('\n') }}</pre>
        <h3>{{ t('environment.commands') }}</h3>
        <pre>{{ plan?.commands }}</pre>
        <pre v-if="running" class="live-log" aria-live="polite">{{
          logs || t('environment.installStarting')
        }}</pre>
        <n-alert v-if="error" type="error">{{ error }}</n-alert>
        <div class="actions">
          <n-button v-if="running" @click="cancel">{{ t('environment.cancelInstall') }}</n-button
          ><n-button v-else @click="plan = undefined">{{ t('common.cancel') }}</n-button
          ><n-button v-if="!running" type="primary" @click="install">{{
            t('environment.confirmInstall')
          }}</n-button>
        </div></n-card
      ></n-modal
    >
  </section>
</template>
<style scoped>
.environment-page {
  overflow: auto;
}
.page-heading {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 16px;
  margin-bottom: 24px;
}
.page-heading h1 {
  margin-bottom: 8px;
}
.environment-tools {
  display: flex;
  gap: 12px;
  align-items: center;
  flex-wrap: wrap;
  margin: 22px 0;
}
.environment-grid {
  display: grid;
  grid-template-columns: minmax(0, 1.6fr) minmax(260px, 1fr);
  gap: 22px;
}
.probe-panel,
.host-panel,
.agents-panel {
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 20px;
  background: var(--surface1);
}
h2 {
  font-size: 15px;
  margin: 0 0 18px;
}
h3 {
  font-size: 13px;
}
.probe-row,
.agent-row {
  display: flex;
  justify-content: space-between;
  gap: 20px;
  align-items: center;
  padding: 14px 0;
  border-bottom: 1px solid var(--border);
}
.probe-row:last-child,
.agent-row:last-of-type {
  border-bottom: 0;
}
.probe-row strong {
  font-size: 13px;
}
.probe-row p,
.agent-row p {
  font-size: 12px;
  line-height: 1.6;
  margin: 5px 0;
  overflow-wrap: anywhere;
}
.probe-row p {
  white-space: pre-wrap;
  color: var(--muted);
}
.host-panel .mono {
  font-size: 11px;
  overflow-wrap: anywhere;
}
.host-panel .actions {
  margin-top: 22px;
  flex-wrap: wrap;
}
.host-warning {
  color: #a86b14;
  font-size: 12px;
  line-height: 1.7;
}
.agents-panel {
  margin-top: 22px;
}
.agents-panel > p {
  font-size: 12px;
  margin-bottom: 0;
  line-height: 1.7;
}
.install-plan {
  width: min(800px, calc(100vw - 40px));
  max-height: 90vh;
  overflow: auto;
}
.install-plan pre,
.install-log pre {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  background: var(--surface2);
  padding: 14px;
  border-radius: 6px;
  font-size: 11px;
  max-height: 240px;
  overflow: auto;
  line-height: 1.6;
}
.install-plan .live-log {
  max-height: 200px;
}
.install-plan > .actions {
  justify-content: flex-end;
  margin-top: 20px;
}
.install-log {
  margin-top: 22px;
}
@media (max-width: 1000px) {
  .environment-grid {
    grid-template-columns: 1fr;
  }
  .page-heading {
    align-items: flex-start;
    flex-wrap: wrap;
  }
}
</style>
