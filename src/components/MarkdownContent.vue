<script setup lang="ts">
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import MarkdownIt from 'markdown-it'
import DOMPurify from 'dompurify'
import { openUrl } from '@tauri-apps/plugin-opener'
const props = defineProps<{ text: string }>()
const { t } = useI18n()
const html = ref('')
const error = ref('')
let revision = 0
const markdown = new MarkdownIt({ html: false, linkify: true, breaks: true })
const highlighted = new Map<string, string>()
markdown.renderer.rules.fence = (tokens, index) => {
  const token = tokens[index]
  const lang = token.info.trim().split(/\s+/)[0] || 'text'
  const key = lang + ':' + token.content
  return `<div class="code-block"><span class="code-language">${markdown.utils.escapeHtml(lang)}</span><button type="button" data-copy="${encodeURIComponent(token.content)}">${markdown.utils.escapeHtml(t('chat.copyCode'))}</button>${highlighted.get(key) ?? '<pre><code>' + markdown.utils.escapeHtml(token.content) + '</code></pre>'}</div>`
}
watch(
  () => props.text,
  (text, _previous, onCleanup) => {
    const request = ++revision
    html.value = DOMPurify.sanitize(markdown.render(text))
    const fences = (markdown.parse(text, {}) ?? [])
      .filter((token) => token.type === 'fence' && token.content.length < 50000)
      .slice(0, 20)
    if (!fences.length) return
    const timer = setTimeout(async () => {
      try {
        const shiki = await import('shiki/bundle/web')
        for (const token of fences) {
          if (request !== revision) return
          const raw = token.info.trim().split(/\s+/)[0] || 'text'
          const lang =
            raw in shiki.bundledLanguages ? (raw as keyof typeof shiki.bundledLanguages) : 'text'
          const key = raw + ':' + token.content
          if (!highlighted.has(key)) {
            const rendered = await shiki.codeToHtml(token.content, { lang, theme: 'github-dark' })
            if (request !== revision) return
            if (highlighted.size >= 20) {
              const oldest = highlighted.keys().next().value
              if (oldest) highlighted.delete(oldest)
            }
            highlighted.set(key, rendered)
          }
        }
        if (request === revision) html.value = DOMPurify.sanitize(markdown.render(text))
      } catch {
        /* Plain code remains readable when syntax resources cannot load. */
      }
    }, 120)
    onCleanup(() => {
      clearTimeout(timer)
      revision++
    })
  },
  { immediate: true },
)
async function clicked(event: MouseEvent) {
  const node = event.target as Element
  const button = node.closest<HTMLButtonElement>('button[data-copy]')
  if (button) {
    try {
      await navigator.clipboard.writeText(decodeURIComponent(button.dataset.copy ?? ''))
      button.textContent = t('chat.copied')
    } catch {
      error.value = t('chat.copyFailed')
    }
  }
  const link = node.closest<HTMLAnchorElement>('a')
  if (link) {
    event.preventDefault()
    if (/^https?:\/\//i.test(link.href))
      await openUrl(link.href).catch(() => {
        error.value = t('chat.linkFailed')
      })
  }
}
</script>
<template>
  <div class="markdown-content" @click="clicked" v-html="html" />
  <small v-if="error" role="alert">{{ error }}</small>
</template>
<style scoped>
.markdown-content {
  overflow-wrap: anywhere;
  line-height: 1.75;
  min-width: 0;
}
.markdown-content :deep(p:first-child) {
  margin-top: 0;
}
.markdown-content :deep(p:last-child) {
  margin-bottom: 0;
}
.markdown-content :deep(pre) {
  overflow: auto;
  padding: 36px 16px 14px;
  margin: 12px 0;
  border-radius: 6px;
  background: #101722;
  color: #e3e9f4;
  max-height: 550px;
  font-size: 12px;
  line-height: 1.65;
}
.markdown-content :deep(.code-block) {
  position: relative;
}
.markdown-content :deep(.code-block button) {
  position: absolute;
  top: 8px;
  right: 10px;
  background: #243044;
  color: #e3e9f4;
  border: 1px solid #526179;
  border-radius: 4px;
  padding: 3px 8px;
  font-size: 11px;
}
.markdown-content :deep(.code-language) {
  position: absolute;
  top: 9px;
  left: 16px;
  font-size: 11px;
  color: #adb9c9;
}
.markdown-content :deep(code) {
  font-family: Consolas, monospace;
  font-size: 0.9em;
}
.markdown-content :deep(table) {
  border-collapse: collapse;
  max-width: 100%;
  display: block;
  overflow: auto;
}
.markdown-content :deep(th),
.markdown-content :deep(td) {
  border: 1px solid var(--border);
  padding: 6px 10px;
}
.markdown-content :deep(img) {
  max-width: 100%;
}
.markdown-content :deep(blockquote) {
  border-left: 3px solid var(--border);
  margin-left: 0;
  padding-left: 14px;
  color: var(--muted);
}
</style>
