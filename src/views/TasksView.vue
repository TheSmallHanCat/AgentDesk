<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { call, desktop, errorText } from '../utils/api'
import type { Transfer, Reply } from '../types'
import { target, unwrap } from '../utils/target'
const { t } = useI18n()
const tasks = ref<Transfer[]>([])
const error = ref('')
let unlisten: UnlistenFn | undefined
const size = (n: number) =>
  n >= 1048576 ? `${(n / 1048576).toFixed(1)} MiB` : `${(n / 1024).toFixed(1)} KiB`
async function refresh() {
  if (desktop)
    try {
      tasks.value = await call('transfer_list')
    } catch (e) {
      error.value = errorText(e)
    }
}
async function cancel(task: Transfer) {
  try {
    const request = target(task.target.connectionId, task.target.projectId, task.target.deviceLocal)
    unwrap(
      request,
      await call<Reply<null>>('transfer_cancel', { target: request, transferId: task.id }),
    )
    await refresh()
  } catch (e) {
    error.value = errorText(e)
  }
}
async function retry(task: Transfer) {
  try {
    const request = target(task.target.connectionId, task.target.projectId, task.target.deviceLocal)
    unwrap(
      request,
      await call<Reply<{ transferId: string }>>('transfer_download', {
        target: request,
        path: task.path,
        destination: task.destination,
      }),
    )
    await refresh()
  } catch (e) {
    error.value = errorText(e)
  }
}
onMounted(async () => {
  if (!desktop) return
  unlisten = await listen<Transfer>('transfer-state', (e) => {
    const i = tasks.value.findIndex((t) => t.id === e.payload.id)
    if (i >= 0) tasks.value[i] = e.payload
    else tasks.value.push(e.payload)
  })
  await refresh()
})
onUnmounted(() => unlisten?.())
</script>
<template>
  <section class="page">
    <div class="page-head">
      <div>
        <h1>{{ t('tasks.title') }}</h1>
        <p class="muted">{{ t('tasks.description') }}</p>
      </div>
      <n-button :disabled="!desktop" @click="refresh">{{ t('common.refresh') }}</n-button>
    </div>
    <p v-if="error" class="error-detail">{{ error }}</p>
    <div v-if="!tasks.length" class="empty panel">
      <h2>{{ t('tasks.empty') }}</h2>
      <p>{{ t('tasks.emptyHelp') }}</p>
    </div>
    <div class="stack">
      <article v-for="task in tasks" :key="task.id" class="panel">
        <div class="page-head" style="margin-bottom: 14px">
          <div>
            <strong class="mono">{{ task.path }}</strong>
            <p class="muted mono">
              {{ task.target.connectionId }} · {{ task.localPath ?? task.destination }}
            </p>
          </div>
          <div class="actions">
            <span>{{ t('state.' + task.state) }}</span
            ><n-button
              v-if="['queued', 'running'].includes(task.state)"
              size="small"
              @click="cancel(task)"
              >{{ t('tasks.cancel') }}</n-button
            ><n-button
              v-if="['failed', 'cancelled'].includes(task.state)"
              size="small"
              @click="retry(task)"
              >{{ t('common.retry') }}</n-button
            >
          </div>
        </div>
        <n-progress
          type="line"
          :percentage="
            task.total
              ? Math.min(100, Math.round((task.transferred / task.total) * 100))
              : task.state === 'completed'
                ? 100
                : 0
          "
        />
        <p class="muted">
          {{ size(task.transferred) }} / {{ size(task.total) }} · {{ size(task.speed) }}/s
        </p>
        <p v-if="task.error" class="error-detail">{{ task.error }}</p>
      </article>
    </div>
  </section>
</template>
