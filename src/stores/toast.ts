import { defineStore } from 'pinia'
import { ref } from 'vue'
export const useToastStore = defineStore('toast', () => {
  const text = ref('')
  const kind = ref<'error' | 'success'>('success')
  let timer: ReturnType<typeof setTimeout> | undefined
  function show(message: string, type: 'error' | 'success' = 'success') {
    clearTimeout(timer)
    text.value = message
    kind.value = type
    timer = setTimeout(() => (text.value = ''), 5000)
  }
  return { text, kind, show }
})
