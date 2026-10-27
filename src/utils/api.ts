import { invoke, isTauri } from '@tauri-apps/api/core'
import type { AppError } from '../types.ts'
import { i18n } from '../i18n/index.ts'
export const desktop = isTauri()
export async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!desktop)
    throw {
      code: 'E_DESKTOP',
      stage: command,
      message: 'Desktop application required',
    } satisfies AppError
  return invoke<T>(command, args)
}
export function errorText(value: unknown): string {
  if (
    typeof value === 'object' &&
    value &&
    'code' in value &&
    i18n.global.te('errors.' + String(value.code))
  )
    return `${String(value.code)} · ${i18n.global.t('errors.' + String(value.code))}`
  if (typeof value === 'object' && value && 'message' in value)
    return `${'code' in value ? String(value.code) + ' · ' : ''}${String(value.message)}${'stage' in value ? ' · ' + String(value.stage) : ''}${'details' in value && value.details && typeof value.details === 'object' && 'hint' in value.details && value.details.hint ? ' · ' + String(value.details.hint) : ''}`
  return String(value)
}
