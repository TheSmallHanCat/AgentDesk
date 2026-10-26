<script setup lang="ts">
import { computed, ref, watch, onUnmounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { FolderOpenOutline } from '@vicons/ionicons5'
import { useDataStore } from '../stores/data'
import { useWorkspaceStore } from '../stores/workspace'
import FileTree from '../components/FileTree.vue'
import ChatPanel from '../components/ChatPanel.vue'
import SessionCreateDialog from '../components/SessionCreateDialog.vue'
import { useChatStore, projectKey } from '../stores/chat'
import { errorText } from '../utils/api'
import type { Project } from '../types'
const route = useRoute()
const router = useRouter()
const { t } = useI18n()
const data = useDataStore()
const workspace = useWorkspaceStore()
const chat = useChatStore()
const terminalOptions = computed(() =>
  Object.entries(chat.agents[project.value?.connectionId ?? ''] ?? {})
    .filter(([, caps]) => caps.nativeTerminal)
    .map(([id, caps]) => ({ label: caps.name ?? id, key: id, disabled: !caps.installed })),
)
const creating = ref<Project>()
const renaming = ref<{ project: Project; id: string }>()
const renameTitle = ref('')
const deleting = ref<{ project: Project; id: string; title: string; agent: string }>()
const actionError = ref('')
function archivedChanged(p: Project, event: Event) {
  chat.archived[projectKey(p)] = (event.target as HTMLInputElement).checked
  void chat.refresh(p)
}
function navigateSession(p: Project, id: string) {
  void router.push('/workbench/' + p.id + '/' + encodeURIComponent(id))
  selectedSession(p, id)
}
function selectedSession(p: Project, id: string) {
  void chat.select(p, id)
}
function renameSession(p: Project, id: string, title: string) {
  renaming.value = { project: p, id }
  renameTitle.value = title
}
async function saveTitle() {
  if (!renaming.value) return
  try {
    await chat.rename(renaming.value.project, renaming.value.id, renameTitle.value.trim())
    renaming.value = undefined
  } catch (error) {
    actionError.value = errorText(error)
  }
}
async function deleteSession() {
  if (!deleting.value) return
  try {
    await chat.remove(deleting.value.project, deleting.value.id)
    deleting.value = undefined
  } catch (error) {
    actionError.value = errorText(error)
  }
}
function activity(value: string) {
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  }).format(new Date(value))
}
const files = ref(window.innerWidth >= 1180)
const projects = ref(true)
const query = ref('')
const width = ref(260)
const project = computed(() => data.projects.find((p) => p.id === route.params.projectId))
const connection = computed(() =>
  data.connections.find((c) => c.id === project.value?.connectionId),
)
const listed = computed(() =>
  data.projects.filter(
    (p) =>
      !p.archived &&
      (p.name.toLowerCase().includes(query.value.toLowerCase()) ||
        chat
          .list(p)
          .some((session) => session.title.toLowerCase().includes(query.value.toLowerCase()))),
  ),
)
watch(
  () => [
    route.params.sessionId,
    project.value
      ? chat
          .list(project.value)
          .map((session) => session.id)
          .join(',')
      : '',
  ],
  ([id]) => {
    const p = project.value
    if (
      p &&
      typeof id === 'string' &&
      chat.list(p).some((session) => session.id === id) &&
      chat.selected(p)?.id !== id
    )
      void chat.select(p, id)
  },
  { immediate: true },
)
let refreshRevision = 0
watch(
  project,
  (p) => {
    workspace.projectId = p?.id ?? ''
    if (p) localStorage.setItem('agentdesk-last-project', p.id)
  },
  { immediate: true },
)
watch(
  () => [
    data.projects.map((p) => p.id).join(','),
    Object.values(data.statuses)
      .map((s) => s.state)
      .join(','),
  ],
  async () => {
    const request = ++refreshRevision
    const online = data.projects.filter((p) => !p.archived && data.online(p.connectionId))
    for (let offset = 0; offset < online.length; offset += 3) {
      if (request !== refreshRevision) break
      await Promise.all(online.slice(offset, offset + 3).map((p) => chat.refresh(p)))
    }
  },
  { immediate: true },
)
function resized() {
  if (window.innerWidth < 1180) files.value = false
}
window.addEventListener('resize', resized)
onUnmounted(() => window.removeEventListener('resize', resized))
let stopDrag: (() => void) | undefined
function resizeProjects(event: PointerEvent) {
  event.preventDefault()
  const start = event.clientX
  const initial = width.value
  const move = (e: PointerEvent) =>
    (width.value = Math.max(180, Math.min(360, initial + e.clientX - start)))
  const stop = () => {
    window.removeEventListener('pointermove', move)
    window.removeEventListener('pointerup', stop)
  }
  stopDrag = stop
  window.addEventListener('pointermove', move)
  window.addEventListener('pointerup', stop, { once: true })
}
onUnmounted(() => stopDrag?.())
</script>
<template>
  <section class="workbench">
    <aside v-if="projects" class="project-panel" :style="{ width: width + 'px' }">
      <div class="project-tools">
        <n-input v-model:value="query" size="small" :placeholder="t('common.search')" /><n-button
          size="tiny"
          @click="projects = false"
          >‹</n-button
        >
      </div>
      <label v-if="project" class="muted"
        ><input
          type="checkbox"
          :checked="chat.archived[projectKey(project)]"
          @change="archivedChanged(project, $event)"
        />{{ t('chat.archived') }}</label
      >
      <section v-for="p in listed" :key="p.id" class="project-group">
        <div class="project-group-head">
          <router-link :to="'/workbench/' + p.id" class="project-link"
            ><FolderOpenOutline />
            <div>
              <strong>{{ p.pinned ? '★ ' : '' }}{{ p.name }}</strong
              ><small>{{ data.connections.find((c) => c.id === p.connectionId)?.name }}</small>
            </div></router-link
          ><n-button
            text
            size="tiny"
            :aria-label="t('chat.newSession')"
            :disabled="!data.online(p.connectionId)"
            @click="creating = p"
            >＋</n-button
          >
        </div>
        <button
          v-for="session in chat
            .list(p)
            .filter(
              (value) =>
                !query ||
                p.name.toLowerCase().includes(query.toLowerCase()) ||
                value.title.toLowerCase().includes(query.toLowerCase()),
            )"
          :key="session.id"
          class="session-link"
          :class="{ active: project?.id === p.id && chat.active[projectKey(p)] === session.id }"
          @click="navigateSession(p, session.id)"
        >
          <span class="agent-mark" :title="session.agent">{{
            session.agent.slice(0, 1).toUpperCase()
          }}</span
          ><span class="session-copy"
            ><strong>{{ session.title }}</strong
            ><small>{{ activity(session.updatedAt) }}</small></span
          ><span class="session-dot" :class="session.status" />
          <n-dropdown
            trigger="click"
            :options="[
              { key: 'rename', label: t('common.rename') },
              ...(session.archived ? [{ key: 'restore', label: t('projects.restore') }] : []),
              {
                key: 'delete',
                label: t(session.agent === 'fake' ? 'common.delete' : 'chat.archiveSession'),
              },
            ]"
            @select="
              (key) =>
                key === 'restore'
                  ? chat.restore(p, session.id)
                  : key === 'rename'
                    ? renameSession(p, session.id, session.title)
                    : (deleting = {
                        project: p,
                        id: session.id,
                        title: session.title,
                        agent: session.agent,
                      })
            "
            ><span
              class="session-menu"
              role="button"
              tabindex="0"
              :aria-label="t('chat.sessionActions')"
              @click.stop
              >⋯</span
            ></n-dropdown
          >
        </button>
      </section>
      <router-link to="/projects" class="manage-projects">{{ t('projects.new') }}</router-link>
      <div class="project-resize" role="separator" @pointerdown="resizeProjects" />
    </aside>
    <div class="chat-area">
      <template v-if="project"
        ><header class="workbench-head">
          <div class="target-copy">
            <div class="actions">
              <n-button v-if="!projects" size="tiny" @click="projects = true">›</n-button
              ><strong>{{ project.name }}</strong
              ><span class="status" :class="data.statuses[project.connectionId]?.state">{{
                t('state.' + (data.statuses[project.connectionId]?.state ?? 'disconnected'))
              }}</span>
            </div>
            <p class="mono muted">
              {{ connection?.name }} · {{ connection?.user }}@{{ connection?.host }} :
              {{ project.path }}
            </p>
          </div>
          <div class="actions">
            <n-button
              size="small"
              :disabled="!project.deviceLocal && !data.online(project.connectionId)"
              @click="workspace.openTerminal(project)"
              >{{ t('workbench.shell') }}</n-button
            ><n-dropdown
              trigger="click"
              :options="terminalOptions"
              @select="(kind: string) => workspace.openTerminal(project!, kind)"
              ><n-button
                size="small"
                :disabled="!data.online(project.connectionId) || !terminalOptions.length"
                >{{ t('workbench.terminal') }} ▾</n-button
              ></n-dropdown
            ><n-button size="small" @click="files = !files">{{ t('workbench.files') }}</n-button
            ><n-button size="small" @click="workspace.dockOpen = !workspace.dockOpen"
              >{{ t('workbench.terminal') }} ↕</n-button
            >
          </div>
        </header>
        <n-alert v-if="!data.online(project.connectionId)" type="warning" class="connection-warning"
          >{{ t('workbench.disconnected') }}
          <router-link to="/connections">{{ t('nav.connections') }}</router-link></n-alert
        >
        <n-alert v-if="chat.errors[projectKey(project)]" type="warning" class="connection-warning"
          >{{ chat.errors[projectKey(project)] }}
          <router-link to="/environment">{{ t('environment.title') }}</router-link
          ><n-button text size="small" @click="chat.refresh(project)">{{
            t('common.refresh')
          }}</n-button></n-alert
        >
        <ChatPanel :project="project" @create="creating = project"
      /></template>
      <div v-else class="empty no-project">
        <FolderOpenOutline />
        <h2>{{ t('workbench.noProject') }}</h2>
        <p>{{ t('workbench.noProjectHelp') }}</p>
        <router-link to="/projects">{{ t('nav.projects') }}</router-link>
      </div>
    </div>
    <div v-if="project && files" class="file-panel">
      <FileTree
        v-if="project.deviceLocal || data.online(project.connectionId)"
        :project="project"
        :key="project.id"
      />
      <div v-else class="empty">{{ t('workbench.disconnected') }}</div>
    </div>
    <SessionCreateDialog :project="creating" @close="creating = undefined" />
    <n-modal :show="!!renaming" @update:show="(value) => !value && (renaming = undefined)"
      ><n-card style="width: 420px" :title="t('chat.renameSession')"
        ><n-input v-model:value="renameTitle" :maxlength="256" />
        <p v-if="actionError" role="alert">{{ actionError }}</p>
        <n-button type="primary" :disabled="!renameTitle.trim()" @click="saveTitle">{{
          t('common.save')
        }}</n-button></n-card
      ></n-modal
    >
    <n-modal :show="!!deleting" @update:show="(value) => !value && (deleting = undefined)"
      ><n-card
        style="width: 440px"
        :title="t(deleting?.agent === 'fake' ? 'chat.deleteSession' : 'chat.archiveSession')"
        ><p>{{ t('chat.deleteConfirm', { title: deleting?.title }) }}</p>
        <p v-if="actionError" role="alert">{{ actionError }}</p>
        <div class="actions">
          <n-button @click="deleting = undefined">{{ t('common.cancel') }}</n-button
          ><n-button type="error" @click="deleteSession">{{
            t(deleting?.agent === 'fake' ? 'common.delete' : 'chat.archiveSession')
          }}</n-button>
        </div></n-card
      ></n-modal
    >
  </section>
</template>
<style scoped>
.workbench {
  height: 100%;
  display: flex;
  overflow: hidden;
  min-width: 0;
}
.project-panel {
  flex: none;
  position: relative;
  background: var(--surface1);
  border-right: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  min-width: 0;
  padding: 14px 10px;
  overflow: auto;
}
.project-tools {
  display: flex;
  gap: 6px;
  margin-bottom: 14px;
}
.project-link {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 12px;
  border-radius: 7px;
  color: var(--text);
}
.project-link svg {
  width: 18px;
  color: var(--muted);
}
.project-link small {
  display: block;
  color: var(--muted);
  font-size: 11px;
  margin-top: 4px;
}
.project-link.router-link-active {
  background: var(--surface2);
  color: var(--primary);
}
.manage-projects {
  margin-top: auto;
  padding: 14px;
  font-size: 12px;
}
.project-resize {
  position: absolute;
  top: 0;
  bottom: 0;
  right: -3px;
  width: 6px;
  cursor: col-resize;
  z-index: 2;
}
.chat-area {
  display: flex;
  flex-direction: column;
  min-width: 0;
  flex: 1;
}
.workbench-head {
  min-height: 92px;
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 18px 22px;
  gap: 12px;
  border-bottom: 1px solid var(--border);
  background: var(--surface1);
  flex-wrap: wrap;
}
.target-copy {
  min-width: 0;
  flex: 1;
}
.target-copy p {
  font-size: 11px;
  margin: 8px 0 0;
  overflow-wrap: anywhere;
}
.agent-empty {
  flex: 1;
}
.agent-empty h2 {
  font-size: 18px;
}
.execution-label {
  border: 1px solid var(--border);
  border-radius: 5px;
  padding: 7px 10px;
  font-size: 11px;
  margin-top: 14px;
}
.no-project {
  flex: 1;
}
.file-panel {
  flex: none;
  width: 290px;
  min-width: 0;
}
.connection-warning {
  margin: 12px;
  width: calc(100% - 24px);
}
@media (max-width: 1179px) {
  .file-panel {
    position: absolute;
    right: 0;
    top: 40px;
    bottom: 0;
    width: 300px;
    z-index: 15;
    box-shadow: -8px 0 26px #0002;
  }
  .workbench-head {
    padding: 15px;
  }
  .project-panel {
    max-width: 220px;
  }
}
.project-group {
  margin-bottom: 12px;
}
.project-group-head {
  display: flex;
  align-items: center;
  gap: 6px;
}
.project-group-head .project-link {
  min-width: 0;
  flex: 1;
  padding: 10px 6px;
}
.project-group-head strong {
  font-size: 12px;
}
.session-link {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  border: 0;
  text-align: left;
  color: var(--text);
  background: transparent;
  border-radius: 6px;
  padding: 9px 8px;
  margin: 2px 0;
}
.session-link:hover,
.session-link.active {
  background: var(--surface2);
}
.session-link.active {
  color: var(--primary);
}
.agent-mark {
  border: 1px solid var(--border);
  border-radius: 5px;
  width: 24px;
  height: 24px;
  display: grid;
  place-items: center;
  font-size: 11px;
  flex: none;
}
.session-copy {
  display: block;
  min-width: 0;
  flex: 1;
}
.session-copy strong {
  display: block;
  font-weight: 500;
  font-size: 12px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.session-copy small {
  display: block;
  color: var(--muted);
  font-size: 10px;
  margin-top: 4px;
}
.session-dot {
  height: 6px;
  width: 6px;
  border-radius: 50%;
  background: var(--muted);
  flex: none;
}
.session-dot.running {
  background: #388c60;
}
.session-dot.awaiting_approval {
  background: #c38b24;
}
.session-menu {
  padding: 2px 5px;
  color: var(--muted);
}
</style>
