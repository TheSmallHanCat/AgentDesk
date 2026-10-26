import { invoke, isTauri } from '@tauri-apps/api/core'
import type { AppError } from '../types.ts'
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
  if (typeof value === 'object' && value && 'message' in value)
    return `${'code' in value ? String(value.code) + ' · ' : ''}${String(value.message)}`
  return String(value)
}
