<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { Channel } from '@tauri-apps/api/core'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { SearchAddon } from '@xterm/addon-search'
import { WebLinksAddon } from '@xterm/addon-web-links'
import { Unicode11Addon } from '@xterm/addon-unicode11'
import { openUrl } from '@tauri-apps/plugin-opener'
import type { TerminalTab } from '../stores/workspace'
import type { TerminalFrame, Reply } from '../types'
import { call, errorText } from '../utils/api'
import { sameTarget, unwrap } from '../utils/target'
import { useSettingsStore } from '../stores/settings'
import { themeIsDark } from '../utils/theme'
import { TerminalAcknowledgements } from '../utils/terminal-ack'
import '@xterm/xterm/css/xterm.css'
const props = defineProps<{ tab: TerminalTab; visible: boolean }>()
const { t } = useI18n()
const settings = useSettingsStore()
const element = ref<HTMLDivElement>()
const search = ref('')
const scrolled = ref(false)
function copySelection() {
  void navigator.clipboard
    .writeText(term.getSelection())
    .catch((e) => (props.tab.error = errorText(e)))
}
let term: Terminal
const fit = new FitAddon()
const finder = new SearchAddon()
let observer: ResizeObserver
let timer: ReturnType<typeof setTimeout> | undefined
let raf = 0
let alive = true
let frames: TerminalFrame[] = []
let resourceId = props.tab.terminalId
const acknowledgements = new TerminalAcknowledgements(
  (seq) => call('term_ack', { target: props.tab.target, terminalId: resourceId, seq }),
  (error) => {
    props.tab.error = errorText(error)
  },
)
function terminalTheme() {
  return themeIsDark.value
    ? {
        background: '#111317',
        foreground: '#c9cdd4',
        cursor: '#b7ccee',
        selectionBackground: '#3c5074',
        black: '#23272e',
        red: '#e78c8c',
        green: '#8dc99d',
        yellow: '#e6c684',
        blue: '#7c9ed9',
        magenta: '#cc9acb',
        cyan: '#86c9d8',
        white: '#c9cdd4',
        brightBlack: '#969da8',
        brightRed: '#ffa5a5',
        brightGreen: '#b1e5b2',
        brightYellow: '#f5dfa5',
        brightBlue: '#afc9f4',
        brightMagenta: '#e5b5df',
        brightCyan: '#b1e5ee',
        brightWhite: '#f4f6fa',
      }
    : {
        background: '#fbfcff',
        foreground: '#172033',
        cursor: '#2454ad',
        selectionBackground: '#d5e2fa',
        black: '#172033',
        red: '#b4233a',
        green: '#247447',
        yellow: '#8e6600',
        blue: '#2454ad',
        magenta: '#9d4392',
        cyan: '#176f86',
        white: '#dde3ee',
        brightBlack: '#596579',
        brightRed: '#d44755',
        brightGreen: '#348752',
        brightYellow: '#a87500',
        brightBlue: '#3f70d4',
        brightMagenta: '#bb5fa6',
        brightCyan: '#24889f',
        brightWhite: '#f4f6fa',
      }
}
function resize() {
  clearTimeout(timer)
  timer = setTimeout(() => {
    if (!alive || !element.value?.clientWidth || !element.value.clientHeight) return
    try {
      fit.fit()
      if (props.tab.terminalId && !props.tab.exited)
        void call('term_resize', {
          target: props.tab.target,
          terminalId: props.tab.terminalId,
          cols: term.cols,
          rows: term.rows,
        }).catch((e) => (props.tab.error = errorText(e)))
    } catch (e) {
      props.tab.error = errorText(e)
    }
  }, 100)
}
function flush() {
  raf = 0
  const batch = frames
  frames = []
  if (!batch.length || !alive) return
  const bytes = new Uint8Array(batch.reduce((n, f) => n + f.data.length, 0))
  let offset = 0
  for (const frame of batch) {
    bytes.set(frame.data, offset)
    offset += frame.data.length
  }
  term.write(bytes, () => {
    if (!alive) return
    const last = batch[batch.length - 1]
    acknowledgements.acknowledge(last.seq)
    if (batch.some((frame) => frame.exited)) props.tab.exited = true
  })
}
onMounted(async () => {
  term = new Terminal({
    cursorBlink: true,
    scrollback: settings.values.scrollback,
    fontSize: settings.values.fontSize,
    fontFamily: '"Cascadia Mono", Consolas, monospace',
    theme: terminalTheme(),
    allowProposedApi: true,
    convertEol: false,
  })
  term.loadAddon(fit)
  term.loadAddon(finder)
  term.loadAddon(new Unicode11Addon())
  term.unicode.activeVersion = '11'
  term.loadAddon(
    new WebLinksAddon((_e, url) => {
      if (/^https?:\/\//i.test(url))
        void openUrl(url).catch((e) => (props.tab.error = errorText(e)))
    }),
  )
  term.open(element.value!)
  fit.fit()
  term.onData((data) => {
    if (props.tab.terminalId && !props.tab.exited)
      void call('term_write', {
        target: props.tab.target,
        terminalId: props.tab.terminalId,
        data,
      }).catch((e) => (props.tab.error = errorText(e)))
  })
  term.onScroll(() => (scrolled.value = term.buffer.active.viewportY < term.buffer.active.baseY))
  term.attachCustomKeyEventHandler((e) => {
    if (e.ctrlKey && e.shiftKey && e.code === 'KeyC' && term.hasSelection()) {
      if (e.type === 'keydown') void navigator.clipboard.writeText(term.getSelection())
      return false
    }
    return true
  })
  observer = new ResizeObserver(resize)
  observer.observe(element.value!)
  if (props.tab.terminalId || props.tab.exited) {
    if (props.tab.terminalId) {
      try {
        await call('term_close', { target: props.tab.target, terminalId: props.tab.terminalId })
      } catch (error) {
        props.tab.error = errorText(error)
      }
    }
    props.tab.exited = true
    return
  }
  const output = new Channel<TerminalFrame>()
  output.onmessage = (frame) => {
    if (!alive || !sameTarget(frame.target, props.tab.target)) return
    if (resourceId && resourceId !== frame.terminalId) return
    resourceId = frame.terminalId
    frames.push(frame)
    if (!raf) raf = requestAnimationFrame(flush)
  }
  try {
    const reply = unwrap(
      props.tab.target,
      await call<Reply<{ terminalId: string }>>('term_open', {
        target: props.tab.target,
        path: props.tab.path,
        kind: props.tab.kind,
        cols: term.cols,
        rows: term.rows,
        onOutput: output,
      }),
    )
    resourceId = reply.terminalId
    props.tab.terminalId = reply.terminalId
    if (!alive) {
      await call('term_close', { target: props.tab.target, terminalId: reply.terminalId })
      return
    }
    if (props.visible) term.focus()
    resize()
  } catch (e) {
    props.tab.error = errorText(e)
    props.tab.exited = true
  }
})
watch(
  () => props.visible,
  (visible) => {
    if (visible) {
      resize()
      term?.focus()
    }
  },
)
watch(
  () => [settings.values.scrollback, settings.values.fontSize, themeIsDark.value],
  () => {
    if (term) {
      term.options.scrollback = settings.values.scrollback
      term.options.fontSize = settings.values.fontSize
      term.options.theme = terminalTheme()
      resize()
    }
  },
)
onUnmounted(() => {
  alive = false
  acknowledgements.dispose()
  clearTimeout(timer)
  cancelAnimationFrame(raf)
  observer?.disconnect()
  term?.dispose()
  if (props.tab.terminalId) {
    void call('term_close', { target: props.tab.target, terminalId: props.tab.terminalId }).catch(
      (error) => {
        props.tab.error = errorText(error)
      },
    )
  }
})
</script>
<template>
  <div class="terminal-view">
    <div class="terminal-tools">
      <n-input
        v-model:value="search"
        size="tiny"
        :placeholder="t('workbench.searchTerminal')"
        @keydown.enter="finder.findNext(search)"
        style="max-width: 220px"
      /><n-button size="tiny" @click="finder.findNext(search)">{{
        t('workbench.findNext')
      }}</n-button
      ><n-button size="tiny" @click="copySelection">{{ t('common.copy') }}</n-button
      ><span v-if="tab.exited" class="muted">{{ t('workbench.exited') }}</span
      ><n-button v-if="scrolled" size="tiny" @click="term.scrollToBottom()">{{
        t('workbench.latest')
      }}</n-button>
    </div>
    <p v-if="tab.kind !== 'shell'" class="native-note muted">{{ t('workbench.nativeSession') }}</p>
    <p v-if="tab.error" class="error-detail" role="alert">{{ tab.error }}</p>
    <div ref="element" class="terminal-element" />
  </div>
</template>
<style scoped>
.terminal-view {
  height: 100%;
  display: flex;
  flex-direction: column;
  min-height: 0;
  background: var(--surface1);
}
.terminal-tools {
  display: flex;
  gap: 8px;
  align-items: center;
  padding: 6px 8px;
  border-bottom: 1px solid var(--border);
  flex-wrap: wrap;
}
.terminal-element {
  flex: 1;
  min-height: 0;
  padding: 8px 0 0 8px;
  overflow: hidden;
}
.native-note,
.error-detail {
  margin: 0;
  padding: 4px 10px;
  font-size: 11px;
}
.terminal-element :deep(.xterm) {
  height: 100%;
}
</style>
