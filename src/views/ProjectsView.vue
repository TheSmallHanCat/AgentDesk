<script setup lang="ts">
import { ref, reactive, computed, watch, onUnmounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { FolderOpenOutline } from '@vicons/ionicons5'
import { useDataStore } from '../stores/data'
import { useToastStore } from '../stores/toast'
import { useLoginStore } from '../stores/login'
import { call, desktop, errorText } from '../utils/api'
import { RequestGuard, unwrap } from '../utils/target'
import type { Project, Reply } from '../types'
const { t } = useI18n()
const data = useDataStore()
const toast = useToastStore()
const login = useLoginStore()
const modal = ref(false)
const busy = ref(false)
const error = ref('')
const search = ref('')
const archived = ref(false)
const suggestions = ref<string[]>([])
const recent = ref<string[]>([])
const blank = (): Project => ({
  deviceLocal: false,
  id: '',
  connectionId:
    data.connections.find((c) => data.online(c.id))?.id ?? data.connections[0]?.id ?? '',
  name: '',
  path: '/',
  description: '',
  defaultAgent: '',
  defaultModel: '',
  permissionMode: 'default',
  pinned: false,
  archived: false,
})
const form = reactive<Project>(blank())
const agentOptions = computed(() =>
  Object.entries(login.agents[form.connectionId] ?? {}).map(([id, caps]) => ({
    label: caps.name ?? id,
    value: id,
    disabled: !caps.installed,
  })),
)
watch(
  () => [modal.value, form.connectionId] as const,
  async ([open, id]) => {
    if (!open || !id || !data.online(id)) return
    try {
      await login.refresh(id)
    } catch (value) {
      if (modal.value && form.connectionId === id) error.value = errorText(value)
    }
  },
)
const guard = new RequestGuard()
const recentGuard = new RequestGuard()
let timer: ReturnType<typeof setTimeout> | undefined
const groups = computed(() =>
  data.connections
    .flatMap((connection) =>
      [false, true].map((local) => ({
        connection,
        local,
        projects: data.projects
          .filter(
            (p) =>
              p.connectionId === connection.id &&
              !!p.deviceLocal === local &&
              p.archived === archived.value &&
              `${p.name} ${p.path} ${p.description}`
                .toLowerCase()
                .includes(search.value.toLowerCase()),
          )
          .sort((a, b) => Number(b.pinned) - Number(a.pinned)),
      })),
    )
    .filter((g) => g.projects.length),
)
const options = computed(() =>
  data.connections.map((c) => ({
    label: c.name,
    value: c.id,
    disabled: !form.deviceLocal && !data.online(c.id),
  })),
)
function edit(p?: Project) {
  Object.assign(form, blank(), p)
  error.value = ''
  modal.value = true
}
async function pickLocal() {
  const path = await call<string | null>('project_pick_local')
  if (path) {
    form.path = path
    if (!form.name) form.name = path.split('/').at(-1) ?? ''
  }
}
async function save(createDir = false) {
  if (!form.defaultAgent) return
  busy.value = true
  error.value = ''
  try {
    await call('project_save', { project: { ...form }, createDir })
    await data.refresh()
    modal.value = false
  } catch (e) {
    if (
      !createDir &&
      (e as { code?: string })?.code === 'E_NOT_FOUND' &&
      window.confirm(`${t('projects.createHelp')}\n\n${form.path}`)
    ) {
      await save(true)
      return
    }
    error.value = errorText(e)
  } finally {
    busy.value = false
  }
}
async function pin(p: Project) {
  try {
    await call('project_save', { project: { ...p, pinned: !p.pinned }, createDir: false })
    await data.refresh()
  } catch (e) {
    toast.show(errorText(e), 'error')
  }
}
async function archive(p: Project) {
  if (!window.confirm(`${t('projects.archiveHelp')}\n\n${p.name} · ${p.path}`)) return
  try {
    await call('project_archive', { id: p.id, archived: !p.archived })
    await data.refresh()
  } catch (e) {
    toast.show(errorText(e), 'error')
  }
}
watch(
  () => [form.path, form.connectionId, modal.value] as const,
  () => {
    clearTimeout(timer)
    guard.invalidate()
    suggestions.value = []
    if (
      form.deviceLocal ||
      !modal.value ||
      form.id ||
      !form.connectionId ||
      !data.online(form.connectionId)
    )
      return
    const request = guard.begin(form.connectionId)
    timer = setTimeout(async () => {
      try {
        const reply = await call<Reply<string[]>>('dir_complete', {
          target: request,
          path: form.path,
        })
        if (guard.accepts(reply.target)) suggestions.value = reply.data
      } catch {
        if (guard.current?.requestSeq === request.requestSeq) suggestions.value = []
      }
    }, 180)
  },
)
watch(
  () => [form.connectionId, modal.value] as const,
  async ([id, opened]) => {
    recent.value = []
    recentGuard.invalidate()
    if (form.deviceLocal || !opened || !id || !desktop) return
    const request = recentGuard.begin(id)
    try {
      const reply = await call<Reply<string[]>>('dir_recent', { target: request })
      if (recentGuard.accepts(reply.target)) recent.value = unwrap(request, reply)
    } catch {
      if (recentGuard.current?.requestSeq === request.requestSeq) recent.value = []
    }
  },
)
onUnmounted(() => {
  clearTimeout(timer)
  guard.invalidate()
  recentGuard.invalidate()
})
</script>
<template>
  <section class="page">
    <div class="page-head">
      <div>
        <h1>{{ t('projects.title') }}</h1>
        <p class="muted">{{ t('projects.description') }}</p>
      </div>
      <n-button type="primary" :disabled="!data.connections.length" @click="edit()">{{
        t('projects.new')
      }}</n-button>
    </div>
    <div class="actions" style="margin-bottom: 24px">
      <n-input
        v-model:value="search"
        :placeholder="t('common.search')"
        clearable
        style="width: 320px"
      /><n-checkbox v-model:checked="archived">{{ t('projects.archived') }}</n-checkbox>
    </div>
    <div v-if="!groups.length" class="empty panel">
      <FolderOpenOutline />
      <h2>{{ t('projects.empty') }}</h2>
      <p>{{ t('projects.emptyHelp') }}</p>
      <router-link to="/connections">{{ t('nav.connections') }}</router-link>
    </div>
    <section
      v-for="group in groups"
      :key="group.connection.id + group.local"
      style="margin-bottom: 28px"
    >
      <h2>
        {{ group.local ? t('projects.localDevice') : t('projects.server') }} ·
        {{ group.connection.name }}
        <span class="muted" style="font-size: 12px">{{ group.connection.host }}</span>
      </h2>
      <div class="stack">
        <article v-for="p in group.projects" :key="p.id" class="record">
          <FolderOpenOutline class="record-icon" />
          <div class="record-copy">
            <strong>{{ p.pinned ? '★ ' : '' }}{{ p.name }}</strong>
            <p class="mono muted">{{ p.path }}</p>
            <p v-if="p.description" class="muted">{{ p.description }}</p>
          </div>
          <div class="actions">
            <router-link :to="'/workbench/' + p.id"
              ><n-button type="primary" size="small">{{ t('common.open') }}</n-button></router-link
            ><n-button size="small" @click="pin(p)">{{
              t(p.pinned ? 'projects.unpin' : 'projects.pin')
            }}</n-button
            ><n-button size="small" @click="edit(p)">{{ t('common.edit') }}</n-button
            ><n-button size="small" @click="archive(p)">{{
              t(p.archived ? 'projects.restore' : 'projects.archive')
            }}</n-button>
          </div>
        </article>
      </div>
    </section>
    <n-modal v-model:show="modal" :mask-closable="!busy"
      ><n-card
        style="width: 560px"
        :title="t(form.id ? 'common.edit' : 'projects.new')"
        closable
        @close="modal = false"
        ><n-form label-placement="top">
          <n-form-item :label="t('projects.location')"
            ><n-select
              :value="form.deviceLocal ? 'local' : 'remote'"
              @update:value="form.deviceLocal = $event === 'local'"
              :disabled="!!form.id"
              :options="[
                { label: t('projects.server'), value: 'remote' },
                { label: t('projects.localDevice'), value: 'local' },
              ]"
          /></n-form-item>
          <n-form-item v-if="form.deviceLocal" :label="t('projects.directory')"
            ><n-input :value="form.path" readonly /><n-button
              :disabled="!!form.id"
              @click="pickLocal"
              >{{ t('projects.chooseLocal') }}</n-button
            ></n-form-item
          ><n-form-item :label="t('projects.connection')"
            ><n-select
              v-model:value="form.connectionId"
              :options="options"
              :disabled="!!form.id" /></n-form-item
          ><n-form-item :label="t('common.name')"><n-input v-model:value="form.name" /></n-form-item
          ><n-form-item v-if="!form.deviceLocal" :label="t('projects.directory')"
            ><n-auto-complete
              v-model:value="form.path"
              :options="suggestions.map((value) => ({ label: value, value }))"
              :disabled="!!form.id"
              :get-show="() => !!suggestions.length"
          /></n-form-item>
          <div v-if="recent.length && !form.id" class="actions">
            <span class="muted">{{ t('projects.recent') }}</span
            ><n-button v-for="p in recent" :key="p" size="tiny" @click="form.path = p">{{
              p
            }}</n-button>
          </div>
          <n-form-item :label="t('projects.descriptionField')"
            ><n-input v-model:value="form.description" type="textarea"
          /></n-form-item>
          <div class="field-grid">
            <n-form-item :label="t('projects.defaultAgent')"
              ><n-select v-model:value="form.defaultAgent" :options="agentOptions" /></n-form-item
            ><n-form-item :label="t('projects.defaultModel')"
              ><n-input v-model:value="form.defaultModel"
            /></n-form-item>
          </div>
          <n-form-item :label="t('projects.permissionMode')"
            ><n-select
              v-model:value="form.permissionMode"
              :options="[
                { label: t('projects.approvalDefault'), value: 'default' },
                { label: t('projects.approvalPlan'), value: 'plan', disabled: !!form.deviceLocal },
              ]"
          /></n-form-item>
          <p v-if="form.deviceLocal" class="muted">{{ t('device.planUnavailable') }}</p>
          <p class="muted">{{ t('projects.defaultOnly') }}</p></n-form
        >
        <p v-if="error" class="error-detail" role="alert">{{ error }}</p>
        <template #footer
          ><div class="actions" style="justify-content: flex-end">
            <n-button @click="modal = false">{{ t('common.cancel') }}</n-button
            ><n-button
              type="primary"
              :loading="busy"
              :disabled="
                !form.name.trim() ||
                !form.connectionId ||
                !form.defaultAgent ||
                (!!form.deviceLocal && form.permissionMode === 'plan')
              "
              @click="save()"
              >{{ t('common.save') }}</n-button
            >
          </div></template
        ></n-card
      ></n-modal
    >
  </section>
</template>
