<script setup lang="ts">
import { useI18n } from 'vue-i18n'
import { ConstructOutline } from '@vicons/ionicons5'
const { t } = useI18n()
</script>
<template>
  <section class="page">
    <h1>{{ t('environment.title') }}</h1>
    <div class="empty">
      <ConstructOutline />
      <h2>{{ t('environment.empty') }}</h2>
      <p>{{ t('environment.help') }}</p>
    </div>
  </section>
</template>
