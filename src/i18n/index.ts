import { createI18n } from 'vue-i18n'
import zh from './zh-CN.json' with { type: 'json' }
import en from './en.json' with { type: 'json' }
export const i18n = createI18n({
  legacy: false,
  locale: 'zh-CN',
  fallbackLocale: 'zh-CN',
  flatJson: true,
  messages: { 'zh-CN': zh, en },
})
