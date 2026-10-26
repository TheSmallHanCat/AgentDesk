<script setup lang="ts">
import { ref, watch, onUnmounted, computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { open } from '@tauri-apps/plugin-dialog'
import type { TreeOption } from 'naive-ui'
import { useWorkspaceStore } from '../stores/workspace'
import { useToastStore } from '../stores/toast'
import { call, errorText } from '../utils/api'
import { RequestGuard, target, unwrap } from '../utils/target'
import type { Project, FileEntry, Reply } from '../types'
const props = defineProps<{ project: Project }>()
const { t } = useI18n()
const workspace = useWorkspaceStore()
const toast = useToastStore()
interface Node extends TreeOption {
  entry: FileEntry
  children?: Node[]
}
const nodes = ref<Node[]>([])
const hidden = ref(false)
const currentDir = ref(props.project.path)
const loading = ref(false)
const error = ref('')
const guard = new RequestGuard()
const menu = ref(false)
const x = ref(0)
const y = ref(0)
const selected = ref<FileEntry>()
const operation = ref('')
const modal = ref(false)
const name = ref('')
const operationTarget = ref<Project>()
const operationPath = ref('')
let generation = 0
const options = computed(() =>
  ['newFile', 'newDirectory', 'rename', 'delete', 'copyPath', 'openTerminal', 'download'].map(
    (key) => ({
      key,
      label: t('files.' + key),
      disabled: !selected.value && ['rename', 'delete', 'copyPath', 'download'].includes(key),
    }),
  ),
)
const dirname = (path: string) => path.slice(0, path.lastIndexOf('/')) || '/'
function toNodes(entries: FileEntry[]): Node[] {
  return entries.map((entry) => ({
    key: entry.path,
    label: entry.name + (entry.kind === 'symlink' ? ' ↗' : ''),
    isLeaf: entry.kind !== 'directory',
    entry,
  }))
}
async function refresh() {
  const request = guard.begin(props.project.connectionId, props.project.id)
  loading.value = true
  error.value = ''
  generation++
  try {
    const reply = await call<Reply<FileEntry[]>>('fs_list', {
      target: request,
      path: currentDir.value,
      showHidden: hidden.value,
    })
    if (guard.accepts(reply.target)) nodes.value = toNodes(reply.data)
  } catch (e) {
    if (guard.current?.requestSeq === request.requestSeq) error.value = errorText(e)
  } finally {
    if (guard.current?.requestSeq === request.requestSeq) loading.value = false
  }
}
async function load(node: TreeOption) {
  const n = node as Node
  const project = { ...props.project }
  const request = target(project.connectionId, project.id)
  const version = generation
  try {
    const entries = unwrap(
      request,
      await call<Reply<FileEntry[]>>('fs_list', {
        target: request,
        path: n.entry.path,
        showHidden: hidden.value,
      }),
    )
    if (generation === version && props.project.id === project.id) n.children = toNodes(entries)
  } catch (e) {
    error.value = errorText(e)
  }
}
async function openEntry(entry: FileEntry) {
  const project = { ...props.project }
  try {
    if (entry.kind === 'directory') return
    if (entry.kind === 'symlink') {
      const request = target(project.connectionId, project.id)
      const stat = unwrap(
        request,
        await call<Reply<FileEntry>>('fs_stat', { target: request, path: entry.path }),
      )
      if (stat.kind === 'directory') {
        currentDir.value = entry.path
        return
      }
    }
    await workspace.openFile(project, entry.path)
  } catch (e) {
    error.value = errorText(e)
  }
}
function nodeProps({ option }: { option: TreeOption }) {
  return {
    title: (option as Node).entry.path,
    onDblclick: () => void openEntry((option as Node).entry),
    onContextmenu: (event: MouseEvent) => {
      event.preventDefault()
      event.stopPropagation()
      selected.value = (option as Node).entry
      x.value = event.clientX
      y.value = event.clientY
      menu.value = true
    },
  }
}
function background(event: MouseEvent) {
  event.preventDefault()
  selected.value = undefined
  x.value = event.clientX
  y.value = event.clientY
  menu.value = true
}
async function action(key: string) {
  menu.value = false
  const project = { ...props.project }
  const entry = selected.value
  const directory =
    entry?.kind === 'directory' ? entry.path : entry ? dirname(entry.path) : currentDir.value
  if (key === 'copyPath') {
    try {
      await navigator.clipboard.writeText(entry?.path ?? directory)
    } catch (e) {
      error.value = errorText(e)
    }
    return
  }
  if (key === 'openTerminal') {
    workspace.openTerminal(project, 'shell', directory)
    return
  }
  if (key === 'download' && entry) {
    try {
      const destination = await open({ directory: true, multiple: false })
      if (typeof destination !== 'string') return
      const request = target(project.connectionId, project.id)
      unwrap(
        request,
        await call<Reply<{ transferId: string }>>('transfer_download', {
          target: request,
          path: entry.path,
          destination,
        }),
      )
      toast.show(t('state.queued'))
    } catch (e) {
      error.value = errorText(e)
    }
    return
  }
  if (key === 'delete' && entry) {
    if (!window.confirm(`${t('files.deleteHelp')}\n\n${project.name} · ${entry.path}`)) return
    try {
      const request = target(project.connectionId, project.id)
      unwrap(request, await call<Reply<null>>('fs_delete', { target: request, path: entry.path }))
      if (props.project.id === project.id) await refresh()
    } catch (e) {
      error.value = errorText(e)
    }
    return
  }
  operation.value = key
  operationTarget.value = project
  operationPath.value = key === 'rename' ? (entry?.path ?? '') : directory
  name.value = key === 'rename' ? (entry?.name ?? '') : ''
  modal.value = true
}
async function perform() {
  const project = operationTarget.value
  if (!project) return
  if (
    !name.value.trim() ||
    name.value === '.' ||
    name.value === '..' ||
    /[\/\\\0]/.test(name.value)
  ) {
    error.value = t('files.nameHelp')
    return
  }
  const path =
    operation.value === 'rename'
      ? operationPath.value
      : `${operationPath.value.replace(/\/$/, '')}/${name.value}`
  const request = target(project.connectionId, project.id)
  try {
    const command =
      operation.value === 'rename'
        ? 'fs_rename'
        : operation.value === 'newFile'
          ? 'fs_create'
          : 'fs_mkdir'
    const args: Record<string, unknown> = { target: request, path }
    if (command === 'fs_rename') args.newPath = `${dirname(path).replace(/\/$/, '')}/${name.value}`
    unwrap(request, await call<Reply<null>>(command, args))
    modal.value = false
    if (props.project.id === project.id) await refresh()
  } catch (e) {
    error.value = errorText(e)
  }
}
watch(
  () => [props.project.id, props.project.path] as const,
  () => {
    currentDir.value = props.project.path
    nodes.value = []
    selected.value = undefined
    menu.value = false
    modal.value = false
  },
  { flush: 'sync' },
)
watch(
  () => [props.project.id, currentDir.value, hidden.value] as const,
  () => void refresh(),
  { immediate: true },
)
onUnmounted(() => {
  guard.invalidate()
  generation++
})
</script>
<template>
  <aside class="file-tree" @contextmenu="background">
    <div class="file-toolbar">
      <strong>{{ t('workbench.files') }}</strong
      ><n-button size="tiny" :title="t('files.up')" @click="currentDir = dirname(currentDir)"
        >↑</n-button
      ><n-button size="tiny" :loading="loading" @click="refresh">{{
        t('common.refresh')
      }}</n-button>
    </div>
    <p class="file-path mono" :title="currentDir">{{ currentDir }}</p>
    <n-checkbox v-model:checked="hidden" size="small" style="padding: 0 12px 12px">{{
      t('files.hidden')
    }}</n-checkbox>
    <p v-if="error" class="error-detail file-error" role="alert">{{ error }}</p>
    <n-tree
      :data="nodes"
      :on-load="load"
      :node-props="nodeProps"
      block-line
      :selectable="false"
      expand-on-click
      style="padding: 0 6px"
    />
    <p v-if="!nodes.length && !loading" class="muted file-error">{{ t('files.empty') }}</p>
    <n-dropdown
      :show="menu"
      :x="x"
      :y="y"
      :options="options"
      placement="bottom-start"
      trigger="manual"
      @select="action"
      @clickoutside="menu = false"
    /><n-modal v-model:show="modal"
      ><n-card style="width: 460px" :title="t('files.' + operation)" closable @close="modal = false"
        ><p class="mono">{{ operationPath }}</p>
        <n-input
          v-model:value="name"
          :placeholder="t('files.nameHelp')"
          autofocus
          @keydown.enter="perform"
        /><template #footer
          ><div class="actions">
            <n-button @click="modal = false">{{ t('common.cancel') }}</n-button
            ><n-button type="primary" @click="perform">{{ t('common.confirm') }}</n-button>
          </div></template
        ></n-card
      ></n-modal
    >
  </aside>
</template>
<style scoped>
.file-tree {
  height: 100%;
  min-width: 0;
  overflow: auto;
  background: var(--surface1);
  border-left: 1px solid var(--border);
}
.file-toolbar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 12px;
}
.file-toolbar strong {
  margin-right: auto;
}
.file-path {
  padding: 0 12px;
  margin: 0 0 12px;
  font-size: 11px;
  overflow-wrap: anywhere;
  color: var(--muted);
}
.file-error {
  padding: 0 12px;
  font-size: 12px;
}
</style>
