import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync, readdirSync } from 'node:fs'
import { resolve } from 'node:path'
const zh: Record<string, string> = JSON.parse(
  readFileSync(new URL('../src/i18n/zh-CN.json', import.meta.url), 'utf8'),
)
const en: Record<string, string> = JSON.parse(
  readFileSync(new URL('../src/i18n/en.json', import.meta.url), 'utf8'),
)
test('Chinese and English keys match with nonempty translations', () => {
  assert.deepEqual(Object.keys(zh).sort(), Object.keys(en).sort())
  for (const locale of [zh, en])
    for (const [key, value] of Object.entries(locale))
      assert.ok(typeof value === 'string' && value.trim().length > 0, key)
})
function files(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) =>
    entry.isDirectory()
      ? files(resolve(dir, entry.name))
      : /\.(vue|ts)$/.test(entry.name)
        ? [resolve(dir, entry.name)]
        : [],
  )
}
test('literal translation references exist in both catalogs', () => {
  for (const file of files(resolve('src'))) {
    const content = readFileSync(file, 'utf8')
    for (const match of content.matchAll(/\bt\(['"]([\w.]+)['"]\s*\)/g)) {
      assert.ok(zh[match[1]], `${file}: ${match[1]}`)
      assert.ok(en[match[1]], `${file}: ${match[1]}`)
    }
  }
})
