import { ref } from 'vue'
import type { GlobalThemeOverrides } from 'naive-ui'
export const themeIsDark = ref(false)
const light = { surface: '#f4f6fa', surface1: '#fbfcff', surface2: '#eef2f7', text: '#172033', muted: '#596579', border: '#dfe5ee', primary: '#3f70d4' }
const dark = { surface: '#111317', surface1: '#191c21', surface2: '#23272e', text: '#c9cdd4', muted: '#969da8', border: '#303640', primary: '#7c9ed9' }
export function applyTheme(mode: string) {
  themeIsDark.value = mode === 'dark' || (mode === 'system' && matchMedia('(prefers-color-scheme: dark)').matches)
  const tokens = themeIsDark.value ? dark : light
  document.documentElement.classList.toggle('dark-theme', themeIsDark.value)
  for (const [key, value] of Object.entries(tokens)) document.documentElement.style.setProperty(`--${key}`, value)
  document.documentElement.style.colorScheme = themeIsDark.value ? 'dark' : 'light'
}
export function themeOverrides(): GlobalThemeOverrides {
  const t = themeIsDark.value ? dark : light
  return { common: { primaryColor: t.primary, primaryColorHover: t.primary, primaryColorPressed: t.primary, bodyColor: t.surface, cardColor: t.surface1, modalColor: t.surface1, popoverColor: t.surface1, inputColor: t.surface2, borderColor: t.border, dividerColor: t.border, textColorBase: t.text, textColor1: t.text, textColor2: t.muted, borderRadius: '8px', fontFamily: 'MiSans, "Segoe UI", "Microsoft YaHei UI", sans-serif' }, Button: { borderRadiusMedium: '8px' } }
}
