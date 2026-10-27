<script setup lang="ts">
import { computed, ref, watch, nextTick } from 'vue'
import { useI18n } from 'vue-i18n'
import { useVirtualizer } from '@tanstack/vue-virtual'
import { useChatStore } from '../stores/chat'
import { useDeviceStore } from '../stores/device'
import { useDataStore } from '../stores/data'
import { groupItems, isRunning, type ChatItem } from '../utils/chat'
import { call, errorText } from '../utils/api'
import { target, unwrap } from '../utils/target'
import type { Project, Reply } from '../types'
import type { BlobResult } from '../protocol/generated'
import MarkdownContent from './MarkdownContent.vue'
import ApprovalCard from './ApprovalCard.vue'
import AgentLoginStatus from './AgentLoginStatus.vue'
const props = defineProps<{ project: Project }>()
const emit = defineEmits<{ create: [] }>()
const { t } = useI18n()
const chat = useChatStore()
const data = useDataStore()
const device = useDeviceStore()
const canSend = computed(
  () =>
    !chat.selected(props.project)?.archived &&
    !!chat.agents[props.project.connectionId]?.[chat.selected(props.project)?.agent ?? '']
      ?.loggedIn &&
    !!chat.agents[props.project.connectionId]?.[chat.selected(props.project)?.agent ?? '']
      ?.available &&
    (!props.project.deviceLocal ||
      (device.state(props.project).enabled && device.state(props.project).state === 'ready')),
)
watch(
  () => props.project,
  (p) => {
    if (p.deviceLocal) void device.refresh(p)
  },
  { immediate: true },
)
const session = computed(() => chat.selected(props.project))
const policy = computed(
  () => session.value?.executionPolicy as { sandbox?: string; networkAccess?: boolean } | undefined,
)
const current = computed(() =>
  session.value ? chat.state(props.project, session.value.id) : undefined,
)
const groups = computed(() => groupItems(current.value?.items ?? []))
const pending = computed(
  () =>
    current.value?.pending.filter(
      (message) => !current.value?.items.some((item) => item.id === 'message:' + message.id),
    ) ?? [],
)
const viewport = ref<HTMLDivElement>()
const follow = ref(true)
const loadingBlob = ref<string>()
const virtualizer = useVirtualizer(
  computed(() => ({
    count: groups.value.length,
    getScrollElement: () => viewport.value ?? null,
    estimateSize: () => 160,
    overscan: 5,
    getItemKey: (index: number) => groups.value[index].id,
  })),
)
const rows = computed(() => virtualizer.value.getVirtualItems())
function measure(element: unknown) {
  if (element instanceof Element) virtualizer.value.measureElement(element)
}
function scrollChanged() {
  const el = viewport.value
  if (el) follow.value = el.scrollHeight - el.scrollTop - el.clientHeight < 100
}
async function latest() {
  follow.value = true
  await nextTick()
  if (viewport.value) viewport.value.scrollTop = viewport.value.scrollHeight
}
watch(
  () => current.value?.lastSeq,
  () => {
    if (follow.value) void latest()
  },
  { flush: 'post' },
)
watch(
  () => session.value?.id,
  () => {
    follow.value = true
    void latest()
  },
)
async function older() {
  if (!session.value || !viewport.value) return
  const el = viewport.value
  const height = el.scrollHeight
  await chat.history(props.project, session.value.id, true)
  await nextTick()
  el.scrollTop += el.scrollHeight - height
}
function keyboard(event: KeyboardEvent) {
  if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
    event.preventDefault()
    if (session.value && current.value && current.value.state !== 'stopping' && canSend.value)
      void chat.send(props.project, session.value.id)
  }
}
function printable(value: unknown) {
  return typeof value === 'string' ? value : JSON.stringify(value, null, 2)
}
async function blob(item: ChatItem) {
  if (!session.value) return
  const project = props.project,
    sid = session.value.id
  loadingBlob.value = item.id
  try {
    let offset = 0,
      content = ''
    do {
      const owner = {
        ...target(project.connectionId, project.id, project.deviceLocal),
        sessionId: sid,
      }
      const result = unwrap(
        owner,
        await call<Reply<BlobResult>>('event_blob', {
          target: owner,
          seq: item.blobSeq ?? item.lastSeq,
          offset,
          limit: 131072,
        }),
      )
      if (item.type !== 'approval.request' && result.total > 8 * 1024 * 1024)
        throw new Error(t('chat.outputTooLarge'))
      content += result.text
      offset = result.nextOffset
      if (!result.hasMore) break
    } while (true)
    if (props.project.id !== project.id || session.value?.id !== sid) return
    item.payload = { ...item.payload, ...JSON.parse(content) }
    delete item.payload.blob
    delete item.payload.truncated
  } catch (error) {
    if (current.value && session.value?.id === sid) current.value.error = errorText(error)
  } finally {
    loadingBlob.value = undefined
  }
}
const context = computed(() => {
  const usage = current.value?.usage
  if (!usage) return undefined
  const value = usage.contextPercent ?? usage.contextUsagePercent
  return typeof value === 'number' ? Math.round(value) + '%' : undefined
})
</script>
<template>
  <div class="chat-surface">
    <div v-if="project.deviceLocal" class="device-status" role="status">
      <strong
        >{{ t('device.channel') }} · {{ t('device.state.' + device.state(project).state) }}</strong
      >
      <n-button
        v-if="!device.state(project).enabled"
        size="small"
        :disabled="
          !data.online(project.connectionId) ||
          ['preparing', 'probing', 'revoking'].includes(device.state(project).state)
        "
        @click="device.enable(project)"
        >{{ t('device.enable') }}</n-button
      >
      <n-button v-else size="small" @click="device.revoke(project)">{{
        t('device.revoke')
      }}</n-button>
      <span
        v-if="
          project.deviceLocal &&
          device.state(project).state === 'closed' &&
          current?.deviceState?.reason !== 'grace_expired'
        "
        >{{ t('device.offlineGrace') }}</span
      >
      <span v-if="current?.deviceState?.reason === 'grace_expired'">{{
        t('device.graceExpired')
      }}</span>
      <span v-if="device.state(project).error" role="alert">{{ device.state(project).error }}</span>
    </div>
    <div v-if="!session" class="chat-empty empty">
      <h2>{{ t('chat.startTitle') }}</h2>
      <p>{{ t('chat.startHelp') }}</p>
      <n-button
        :disabled="!data.online(project.connectionId) || !canSend"
        type="primary"
        @click="emit('create')"
        >{{ t('chat.newSession') }}</n-button
      ><router-link to="/environment">{{ t('environment.title') }}</router-link>
    </div>
    <div v-else class="conversation" :aria-label="t('chat.conversation')">
      <header class="conversation-head">
        <div>
          <strong>{{ session.title }}</strong
          ><span class="muted">{{ session.agent }}</span>
          <AgentLoginStatus
            :agent="session.agent"
            :connection-id="project.connectionId"
            :project-id="project.id"
            :session-id="session.id"
            :device-local="project.deviceLocal"
            :capability="chat.agents[project.connectionId]?.[session.agent]"
            :disabled="!data.online(project.connectionId)"
          />
        </div>
        <span class="run-state" aria-live="polite">{{
          t('chat.state.' + (current?.state ?? 'idle'))
        }}</span>
      </header>
      <div ref="viewport" class="message-scroll" @scroll="scrollChanged">
        <div class="history-tools">
          <n-button
            v-if="current?.hasMore"
            size="small"
            :loading="current.loading"
            @click="older"
            >{{ t('chat.loadEarlier') }}</n-button
          ><n-spin v-else-if="current?.loading" size="small" />
        </div>
        <div class="virtual-content" :style="{ height: virtualizer.getTotalSize() + 'px' }">
          <article
            v-for="row in rows"
            :key="String(row.key)"
            :ref="measure"
            :data-index="row.index"
            class="message-row"
            :style="{ transform: `translateY(${row.start}px)` }"
          >
            <template v-if="groups[row.index].type === 'steps'"
              ><details class="work-steps">
                <summary>
                  {{ t('chat.workSteps', { count: groups[row.index].items.length }) }}
                </summary>
                <details v-for="item in groups[row.index].items" :key="item.id" class="tool-step">
                  <summary>
                    {{
                      item.payload.name ??
                      t('chat.kind.' + String(item.payload.kind), String(item.payload.kind))
                    }}
                  </summary>
                  <small
                    v-if="project.deviceLocal && item.payload.kind === 'file_change'"
                    class="muted"
                    >{{ t('device.serverFileChange') }}</small
                  >
                  <pre v-if="item.payload.input">{{ printable(item.payload.input) }}</pre>
                  <pre>{{ printable(item.payload.output ?? item.payload.text) }}</pre>
                  <n-button
                    v-if="item.payload.truncated && item.complete"
                    size="tiny"
                    :loading="loadingBlob === item.id"
                    @click="blob(item)"
                    >{{ t('chat.loadFullOutput') }}</n-button
                  >
                </details>
              </details></template
            >
            <template v-else v-for="item in groups[row.index].items" :key="item.id">
              <div v-if="item.type === 'user.message'" class="user-message">
                <span class="message-author">{{ t('chat.you') }}</span>
                <div>{{ item.payload.text }}</div>
                <small>{{
                  t(
                    item.payload.delivery === 'undelivered'
                      ? 'chat.delivery.undelivered'
                      : 'chat.delivery.delivered',
                  )
                }}</small>
              </div>
              <ApprovalCard
                v-else-if="item.type === 'approval.request'"
                :item="item"
                :load="() => blob(item)"
                :loading="loadingBlob === item.id"
                :submit="(id, decision) => chat.approve(project, session!.id, id, decision)"
              />
              <details v-else-if="item.payload.kind === 'reasoning'" class="reasoning">
                <summary>{{ t('chat.reasoning') }}</summary>
                <MarkdownContent :text="String(item.payload.text ?? '')" />
              </details>
              <div v-else-if="item.payload.kind === 'error'" role="alert" class="message-error">
                <strong>{{ item.payload.code }}</strong>
                <p>{{ item.payload.text }}</p>
                <small>{{ item.payload.stage }} · {{ item.payload.hint }}</small>
              </div>
              <div v-else class="assistant-message">
                <span class="message-author">{{ session.agent }}</span
                ><MarkdownContent :text="String(item.payload.text ?? '')" /><span
                  v-if="!item.complete"
                  class="stream-cursor"
                  aria-hidden="true"
                  >▍</span
                ><n-button
                  v-if="item.payload.truncated && item.complete"
                  size="tiny"
                  :loading="loadingBlob === item.id"
                  @click="blob(item)"
                  >{{ t('chat.loadFullOutput') }}</n-button
                >
              </div>
            </template>
          </article>
        </div>
        <div v-for="message in pending" :key="message.id" class="pending-message user-message">
          <span class="message-author">{{ t('chat.you') }}</span>
          <div>{{ message.text }}</div>
          <small :class="message.status">{{ t('chat.delivery.' + message.status) }}</small>
          <p v-if="message.status === 'unknown'">{{ t('chat.unknownHelp') }}</p>
        </div>
      </div>
      <button v-if="!follow" class="latest-button" @click="latest">↓ {{ t('chat.latest') }}</button>
      <n-alert v-if="current?.error" type="error" closable @close="current.error = ''">{{
        current.error
      }}</n-alert>
      <form class="composer" @submit.prevent="canSend && chat.send(project, session.id)">
        <textarea
          v-if="current"
          v-model="current.draft"
          :aria-label="t('chat.input')"
          :placeholder="t('chat.inputPlaceholder')"
          :disabled="!data.online(project.connectionId) || !canSend"
          rows="3"
          @keydown="keyboard"
        />
        <div class="composer-actions">
          <small class="muted">{{ t('chat.inputHelp') }}</small
          ><n-button
            v-if="current && isRunning(current.state)"
            type="error"
            :disabled="current.state === 'stopping' || !data.online(project.connectionId)"
            @click="chat.stop(project, session.id)"
            >{{ t('chat.stop') }}</n-button
          ><n-button
            attr-type="submit"
            type="primary"
            :disabled="
              !canSend ||
              current?.state === 'stopping' ||
              !current?.draft.trim() ||
              !data.online(project.connectionId) ||
              current?.pending.some((value) => value.status === 'sending')
            "
            >{{ t('chat.send') }}</n-button
          >
        </div>
      </form>
      <footer class="session-status">
        <span v-if="policy?.sandbox"
          >{{ t('device.sandbox') }}: {{ policy.sandbox }} · network_access={{
            policy.networkAccess
          }}</span
        >
        <span>{{ session.model }}</span
        ><span
          >{{ t('chat.permission') }}:
          {{ t('chat.mode.' + session.permissionMode, session.permissionMode) }}</span
        ><span v-if="context">{{ t('chat.context') }} {{ context }}</span
        ><span v-if="project.deviceLocal">{{
          t('device.state.' + device.state(project).state)
        }}</span>
        <span class="execution-target" :title="project.path"
          >{{ t(project.deviceLocal ? 'projects.localDevice' : 'projects.server') }} ·
          {{ data.connections.find((value) => value.id === project.connectionId)?.name }} ·
          {{ project.path }}</span
        >
      </footer>
    </div>
  </div>
</template>
<style scoped>
.chat-surface {
  display: flex;
  flex-direction: column;
  min-height: 0;
  flex: 1;
}
.device-status {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 10px 16px;
  border-bottom: 1px solid var(--border);
}
.conversation {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 0;
  position: relative;
  background: var(--surface);
}
.conversation-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 14px 24px;
  border-bottom: 1px solid var(--border);
  gap: 12px;
}
.conversation-head div {
  display: flex;
  gap: 12px;
  align-items: center;
  min-width: 0;
}
.conversation-head strong {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.conversation-head span {
  font-size: 12px;
}
.run-state {
  color: var(--muted);
  flex: none;
}
.message-scroll {
  overflow: auto;
  flex: 1;
  min-height: 0;
  overflow-anchor: none;
}
.history-tools {
  text-align: center;
  padding: 10px;
  min-height: 20px;
}
.virtual-content {
  position: relative;
  width: 100%;
}
.message-row {
  position: absolute;
  top: 0;
  left: 0;
  width: 100%;
  padding: 0 max(22px, calc((100% - 840px) / 2)) 24px;
}
.message-author {
  display: block;
  font-size: 11px;
  font-weight: 600;
  color: var(--muted);
  margin-bottom: 8px;
  text-transform: uppercase;
  letter-spacing: 0.08em;
}
.user-message {
  white-space: pre-wrap;
  background: var(--surface2);
  padding: 16px 18px;
  border-radius: 8px;
}
.user-message small {
  display: block;
  color: var(--muted);
  font-size: 10px;
  margin-top: 9px;
}
.user-message .unknown,
.user-message .failed {
  color: #bc751a;
}
.pending-message {
  margin: 0 24px 20px;
}
.pending-message p {
  font-size: 12px;
  color: var(--muted);
}
.assistant-message {
  padding: 6px 0;
}
.reasoning {
  color: var(--muted);
  font-size: 13px;
}
.reasoning summary {
  cursor: pointer;
  margin-bottom: 10px;
}
.work-steps {
  border: 1px solid var(--border);
  border-radius: 7px;
  background: var(--surface1);
  padding: 12px 16px;
}
.work-steps > summary {
  font-weight: 600;
  cursor: pointer;
}
.tool-step {
  padding: 10px 0 0;
  margin-top: 8px;
  border-top: 1px solid var(--border);
  font-size: 12px;
}
.tool-step summary {
  cursor: pointer;
  font-family: Consolas, monospace;
}
.tool-step pre {
  white-space: pre-wrap;
  overflow: auto;
  max-height: 350px;
  line-height: 1.6;
}
.message-error {
  border-left: 3px solid #bd4d59;
  padding: 12px 16px;
  background: var(--surface1);
  color: #bd4d59;
}
.stream-cursor {
  color: var(--primary);
}
.composer {
  flex: none;
  margin: 0 20px 10px;
  border: 1px solid var(--border);
  background: var(--surface1);
  border-radius: 9px;
  padding: 12px;
}
.composer:focus-within {
  border-color: var(--primary);
}
.composer textarea {
  resize: vertical;
  max-height: 240px;
  min-height: 68px;
  width: 100%;
  border: 0;
  outline: 0;
  background: transparent;
  color: var(--text);
  line-height: 1.6;
}
.composer-actions {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}
.composer-actions small {
  font-size: 11px;
}
.session-status {
  display: flex;
  gap: 14px;
  padding: 8px 22px 10px;
  border-top: 1px solid var(--border);
  font-size: 11px;
  color: var(--muted);
  align-items: center;
  flex-wrap: wrap;
}
.execution-target {
  margin-left: auto;
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-family: Consolas, monospace;
}
.latest-button {
  position: absolute;
  bottom: 190px;
  left: 50%;
  transform: translateX(-50%);
  border: 1px solid var(--border);
  border-radius: 20px;
  padding: 8px 16px;
  background: var(--surface1);
  color: var(--primary);
  box-shadow: 0 4px 14px #0001;
}
.conversation > .n-alert {
  margin: 0 20px 10px;
  width: calc(100% - 40px);
}
.chat-empty {
  flex: 1;
  gap: 12px;
}
.chat-empty h2 {
  font-size: 20px;
}
@media (max-width: 900px) {
  .message-row {
    padding-left: 14px;
    padding-right: 14px;
  }
  .conversation-head {
    padding: 12px 14px;
  }
  .composer {
    margin-left: 10px;
    margin-right: 10px;
  }
  .session-status {
    padding: 8px 12px;
    gap: 7px;
  }
  .composer-actions small {
    max-width: 180px;
  }
}
</style>
