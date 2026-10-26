<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { call, desktop, errorText } from '../utils/api'
import type { Diagnostic } from '../types'
const { t } = useI18n()
const records = ref<Diagnostic[]>([])
const error = ref('')
async function refresh() {
  if (!desktop) return
  try {
    records.value = await call('diag_list')
  } catch (e) {
    error.value = errorText(e)
  }
}
async function exportLog() {
  try {
    await call('diag_export')
  } catch (e) {
    error.value = errorText(e)
  }
}
onMounted(refresh)
</script>
<template>
  <section class="page">
    <div class="page-head">
      <div>
        <h1>{{ t('diagnostics.title') }}</h1>
        <p class="muted">{{ t('diagnostics.description') }}</p>
      </div>
      <div class="actions">
        <n-button :disabled="!desktop" @click="refresh">{{ t('common.refresh') }}</n-button
        ><n-button :disabled="!desktop" @click="exportLog">{{ t('diagnostics.export') }}</n-button>
      </div>
    </div>
    <p v-if="error" class="error-detail">{{ error }}</p>
    <n-table v-if="records.length" :single-line="false"
      ><thead>
        <tr>
          <th>{{ t('diagnostics.time') }}</th>
          <th>{{ t('projects.connection') }}</th>
          <th>{{ t('diagnostics.stage') }}</th>
          <th>{{ t('diagnostics.duration') }}</th>
          <th>{{ t('diagnostics.result') }}</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="(r, i) in records" :key="i">
          <td>{{ new Date(r.timestamp).toLocaleTimeString() }}</td>
          <td class="mono">{{ r.connectionId }}</td>
          <td>{{ r.stage }}</td>
          <td>{{ r.elapsedMs }} ms</td>
          <td>{{ r.success ? t('diagnostics.success') : r.code }}</td>
        </tr>
      </tbody></n-table
    >
    <div v-else class="empty panel">{{ t('common.noData') }}</div>
  </section>
</template>
