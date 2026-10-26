<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch, nextTick } from 'vue'
import { useI18n } from 'vue-i18n'
import { basicSetup } from 'codemirror'
import { EditorState, Compartment, type Extension } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { javascript } from '@codemirror/lang-javascript'
import { json } from '@codemirror/lang-json'
import { python } from '@codemirror/lang-python'
import { html } from '@codemirror/lang-html'
import { css } from '@codemirror/lang-css'
import { oneDark } from '@codemirror/theme-one-dark'
import { MergeView } from '@codemirror/merge'
import { call, errorText } from '../utils/api'
import { target, unwrap } from '../utils/target'
import { themeIsDark } from '../utils/theme'
import { useWorkspaceStore, type EditorTab } from '../stores/workspace'
import type { Reply, FileDocument } from '../types'
const props = defineProps<{ tab: EditorTab; visible: boolean }>()
const { t } = useI18n()
const workspace = useWorkspaceStore()
const root = ref<HTMLDivElement>()
const diffRoot = ref<HTMLDivElement>()
const busy = ref(false)
const conflict = ref(false)
const comparing = ref(false)
let editor: EditorView
let merge: MergeView | undefined
const theme = new Compartment()
let alive = true
function language(): Extension {
  const ext = props.tab.path.split('.').pop()
  if (['js', 'jsx', 'mjs', 'ts', 'tsx'].includes(ext ?? ''))
    return javascript({
      typescript: ext === 'ts' || ext === 'tsx',
      jsx: ext === 'jsx' || ext === 'tsx',
    })
  if (ext === 'json') return json()
  if (ext === 'py') return python()
  if (ext === 'html' || ext === 'vue') return html()
  if (['css', 'scss'].includes(ext ?? '')) return css()
  return []
}
function createState(content: string) {
  return EditorState.create({
    doc: content,
    extensions: [
      basicSetup,
      language(),
      EditorState.lineSeparator.of(props.tab.document.lineEnding === 'crlf' ? '\r\n' : '\n'),
      theme.of(themeIsDark.value ? oneDark : []),
      EditorView.theme({
        '&': { height: '100%', backgroundColor: 'var(--surface1)' },
        '.cm-scroller': { overflow: 'auto', fontFamily: 'Consolas, monospace', fontSize: '13px' },
        '.cm-gutters': {
          backgroundColor: 'var(--surface2)',
          color: 'var(--muted)',
          borderRight: '1px solid var(--border)',
        },
      }),
      EditorView.updateListener.of((update) => {
        if (update.docChanged) {
          props.tab.content = update.state.sliceDoc()
          props.tab.revision++
        }
      }),
    ],
  })
}
async function saveFile(force = false) {
  if (busy.value) return
  busy.value = true
  props.tab.error = ''
  const content = props.tab.content
  const request = target(
    props.tab.target.connectionId,
    props.tab.target.projectId,
    props.tab.target.deviceLocal,
  )
  try {
    const document = unwrap(
      request,
      await call<Reply<FileDocument>>('fs_write', {
        target: request,
        path: props.tab.path,
        content,
        expectedMtime: props.tab.document.mtime,
        expectedSize: props.tab.document.size,
        force,
      }),
    )
    if (!alive) return
    props.tab.document = document
    props.tab.savedContent = content
    conflict.value = false
    if (document.saveMode === 'inPlace') window.alert(t('editor.inPlaceWarning'))
  } catch (e) {
    if (!alive) return
    if ((e as { code?: string })?.code === 'E_CONFLICT') conflict.value = true
    props.tab.error = errorText(e)
  } finally {
    busy.value = false
  }
}
async function remote() {
  const request = target(
    props.tab.target.connectionId,
    props.tab.target.projectId,
    props.tab.target.deviceLocal,
  )
  return unwrap(
    request,
    await call<Reply<FileDocument>>('fs_read', { target: request, path: props.tab.path }),
  )
}
async function reload() {
  if (!window.confirm(`${t('editor.closeHelp')}\n\n${props.tab.path}`)) return
  try {
    const document = await remote()
    if (!alive) return
    props.tab.document = document
    props.tab.content = document.content
    props.tab.savedContent = document.content
    editor.setState(createState(document.content))
    conflict.value = false
    props.tab.error = ''
  } catch (e) {
    props.tab.error = errorText(e)
  }
}
async function compare() {
  try {
    const document = await remote()
    if (!alive) return
    comparing.value = true
    await nextTick()
    merge?.destroy()
    merge = new MergeView({
      parent: diffRoot.value!,
      a: {
        doc: document.content,
        extensions: [
          basicSetup,
          EditorState.readOnly.of(true),
          EditorView.editable.of(false),
          ...(themeIsDark.value ? [oneDark] : []),
        ],
      },
      b: {
        doc: props.tab.content,
        extensions: [
          basicSetup,
          EditorState.readOnly.of(true),
          EditorView.editable.of(false),
          ...(themeIsDark.value ? [oneDark] : []),
        ],
      },
      highlightChanges: true,
      gutter: true,
    })
  } catch (e) {
    props.tab.error = errorText(e)
  }
}
async function overwrite() {
  if (window.confirm(`${t('editor.overwrite')}\n\n${props.tab.path}`)) await saveFile(true)
}
async function saveLocal() {
  try {
    await call('editor_save_local', { content: props.tab.content })
  } catch (e) {
    props.tab.error = errorText(e)
  }
}
watch(
  () => workspace.saveRequested,
  () => {
    if (props.visible && workspace.panes[workspace.activePane] === props.tab.id) void saveFile()
  },
)
watch(themeIsDark, () =>
  editor?.dispatch({ effects: theme.reconfigure(themeIsDark.value ? oneDark : []) }),
)
watch(
  () => props.visible,
  (visible) => {
    if (visible) {
      editor?.requestMeasure()
      editor?.focus()
    }
  },
)
onMounted(() => {
  editor = new EditorView({ parent: root.value!, state: createState(props.tab.content) })
  if (props.visible) editor.focus()
})
function closeComparison() {
  merge?.destroy()
  merge = undefined
}
onUnmounted(() => {
  alive = false
  editor?.destroy()
  merge?.destroy()
})
</script>
<template>
  <div class="code-editor">
    <div class="editor-tools">
      <span class="mono"
        >{{ tab.document.lineEnding.toUpperCase() }} · UTF-8 ·
        {{ (tab.document.permissions & 511).toString(8) }}</span
      ><span v-if="tab.content !== tab.savedContent" class="muted">{{ t('editor.unsaved') }}</span>
      <div class="actions">
        <n-button size="tiny" :loading="busy" @click="saveFile()">{{ t('common.save') }}</n-button
        ><n-button size="tiny" @click="saveLocal">{{ t('editor.saveLocal') }}</n-button>
      </div>
    </div>
    <div v-if="tab.error" class="editor-error" role="alert">
      <strong>{{ t('editor.saveFailed') }}</strong>
      <p class="error-detail">{{ tab.error }}</p>
    </div>
    <div ref="root" class="editor-root" />
    <n-modal v-model:show="conflict"
      ><n-card style="width: 580px" :title="t('editor.conflict')" closable @close="conflict = false"
        ><p>{{ t('editor.conflictHelp') }}</p>
        <p class="mono">{{ tab.path }}</p>
        <template #footer
          ><div class="actions">
            <n-button @click="reload">{{ t('editor.reload') }}</n-button
            ><n-button @click="compare">{{ t('editor.diff') }}</n-button
            ><n-button type="error" @click="overwrite">{{ t('editor.overwrite') }}</n-button
            ><n-button @click="conflict = false">{{ t('common.cancel') }}</n-button>
          </div></template
        ></n-card
      ></n-modal
    ><n-modal v-model:show="comparing" @after-leave="closeComparison"
      ><n-card
        style="width: 1100px; height: 75vh"
        :title="t('editor.diff')"
        closable
        @close="comparing = false"
        ><div class="diff-labels">
          <span>{{ t('editor.remote') }}</span
          ><span>{{ t('editor.local') }}</span>
        </div>
        <div ref="diffRoot" class="diff-root" /></n-card
    ></n-modal>
  </div>
</template>
<style scoped>
.code-editor {
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
  background: var(--surface1);
}
.editor-tools {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 8px;
  font-size: 11px;
  border-bottom: 1px solid var(--border);
}
.editor-tools .actions {
  margin-left: auto;
}
.editor-root {
  flex: 1;
  min-height: 0;
  overflow: hidden;
}
.editor-error {
  padding: 8px 12px;
  font-size: 12px;
}
.editor-error p {
  margin: 2px 0;
}
.diff-labels {
  display: flex;
  justify-content: space-around;
  padding: 8px;
  background: var(--surface2);
}
.diff-root {
  height: calc(75vh - 160px);
  overflow: auto;
}
.diff-root :deep(.cm-mergeView) {
  height: 100%;
}
.diff-root :deep(.cm-scroller) {
  overflow: auto;
}
</style>
