import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'

test('local write IPC cannot accept a destination supplied by a WebView', () => {
  const commands = readFileSync(
    new URL('../src-tauri/src/commands/sys.rs', import.meta.url),
    'utf8',
  )
  for (const name of ['editor_save_local', 'diag_export']) {
    const signature = commands.match(new RegExp(`pub async fn ${name}\\(([\\s\\S]*?)\\) ->`))?.[1]
    assert.ok(signature, `Missing command ${name}`)
    assert.match(signature, /app: tauri::AppHandle/)
    assert.doesNotMatch(signature, /\b(?:path|destination|file_name|filename)\s*:/)
  }
})
