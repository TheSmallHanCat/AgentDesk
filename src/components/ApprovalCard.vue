<script setup lang="ts">
import { ref, computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { errorText } from '../utils/api'
import type { ChatItem } from '../utils/chat'
import type { AgentQuestion, ApprovalDecision } from '../protocol/generated'
const props = defineProps<{
  item: ChatItem
  submit: (approvalId: string, decision: ApprovalDecision) => Promise<void>
}>()
const { t } = useI18n()
const answers = ref<Record<string, string | string[]>>({})
const free = ref<Record<string, string>>({})
const busy = ref(false)
const error = ref('')
const questions = computed(() =>
  Array.isArray(props.item.payload.questions)
    ? (props.item.payload.questions as AgentQuestion[])
    : [],
)
const canSubmit = computed(() =>
  questions.value.every((question) => {
    const value = answers.value[question.question]
    return (
      !!free.value[question.question]?.trim() || (Array.isArray(value) ? value.length > 0 : !!value)
    )
  }),
)
function choose(question: AgentQuestion, label: string, checked: boolean) {
  const key = question.question
  if (question.multiSelect) {
    const selected = Array.isArray(answers.value[key]) ? (answers.value[key] as string[]) : []
    answers.value[key] = checked
      ? [...selected.filter((value) => value !== label), label]
      : selected.filter((value) => value !== label)
  } else answers.value[key] = label
}
async function respond(behavior: 'allow' | 'deny') {
  busy.value = true
  error.value = ''
  try {
    const resolved = { ...answers.value }
    for (const [key, value] of Object.entries(free.value))
      if (value.trim()) resolved[key] = value.trim()
    await props.submit(String(props.item.payload.approvalId), {
      behavior,
      ...(questions.value.length ? { answers: resolved } : {}),
    })
  } catch (value) {
    error.value = errorText(value)
  } finally {
    busy.value = false
  }
}
</script>
<template>
  <section class="approval-card" :aria-label="t('chat.approval')">
    <header>
      <strong>{{ questions.length ? t('chat.question') : t('chat.approval') }}</strong
      ><span>{{ item.payload.tool }}</span>
    </header>
    <template v-if="!item.complete">
      <pre v-if="item.payload.input">{{ JSON.stringify(item.payload.input, null, 2) }}</pre>
      <fieldset v-for="question in questions" :key="question.question" :disabled="busy">
        <legend>{{ question.question }}</legend>
        <label v-for="option in question.options" :key="option.label"
          ><input
            :type="question.multiSelect ? 'checkbox' : 'radio'"
            :name="item.id + question.question"
            :value="option.label"
            @change="choose(question, option.label, ($event.target as HTMLInputElement).checked)"
          /><span
            >{{ option.label }}<small>{{ option.description }}</small></span
          ></label
        >
        <input
          v-model="free[question.question]"
          class="free-answer"
          :placeholder="t('chat.freeAnswer')"
          :aria-label="t('chat.freeAnswer')"
        />
      </fieldset>
      <div class="actions">
        <n-button size="small" :disabled="busy" @click="respond('deny')">{{
          t('chat.deny')
        }}</n-button
        ><n-button
          type="primary"
          size="small"
          :loading="busy"
          :disabled="!canSubmit"
          @click="respond('allow')"
          >{{ questions.length ? t('chat.submitAnswer') : t('chat.allow') }}</n-button
        >
      </div>
    </template>
    <p v-else class="muted">
      {{ t('chat.resolved') }} ·
      {{ t('chat.' + ((item.payload.decision as ApprovalDecision)?.behavior ?? 'deny')) }}
    </p>
    <p v-if="error" role="alert">{{ error }}</p>
  </section>
</template>
<style scoped>
.approval-card {
  border: 1px solid var(--primary);
  border-radius: 8px;
  padding: 16px;
  background: var(--surface1);
}
header {
  display: flex;
  justify-content: space-between;
  gap: 10px;
  margin-bottom: 10px;
}
header span {
  font-family: Consolas, monospace;
  font-size: 12px;
}
pre {
  white-space: pre-wrap;
  max-height: 220px;
  overflow: auto;
  font-size: 12px;
}
fieldset {
  border: 0;
  padding: 10px 0;
  margin: 0;
}
legend {
  font-weight: 600;
}
label {
  display: flex;
  gap: 9px;
  padding: 8px 0;
  cursor: pointer;
}
label small {
  display: block;
  color: var(--muted);
  font-size: 12px;
  margin-top: 3px;
}
.free-answer {
  width: 100%;
  background: var(--surface);
  color: var(--text);
  border: 1px solid var(--border);
  border-radius: 4px;
  padding: 8px;
}
.actions {
  justify-content: flex-end;
}
</style>
