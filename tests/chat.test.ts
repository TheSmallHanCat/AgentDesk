import test from 'node:test'
import assert from 'node:assert/strict'
import { applyEvents, timeline, groupItems } from '../src/utils/chat.ts'
import type { HostEvent } from '../src/protocol/generated.ts'
const event = (
  seq: number,
  type: string,
  payload: Record<string, unknown>,
  itemId?: string,
): HostEvent => ({
  seq,
  type,
  payload,
  itemId,
  sessionId: 's',
  runId: 'r',
  ts: '2026-10-26T00:00:00Z',
})
test('stream snapshots replace partial text and completed replay cannot duplicate it', () => {
  const state = timeline()
  applyEvents(state, [
    event(1, 'item.started', { kind: 'text' }, 'i'),
    event(2, 'item.delta', { kind: 'text', delta: 'he' }, 'i'),
  ])
  applyEvents(state, [
    event(3, 'item.started', { kind: 'text', text: 'hello', snapshot: true }, 'i'),
  ])
  assert.equal(state.items[0].payload.text, 'hello')
  applyEvents(state, [
    event(4, 'item.completed', { kind: 'text', text: 'hello world' }, 'i'),
    event(4, 'item.completed', { kind: 'text', text: 'hello world' }, 'i'),
  ])
  applyEvents(state, [event(2, 'item.delta', { kind: 'text', delta: 'old' }, 'i')])
  assert.equal(state.items.length, 1)
  assert.equal(state.items[0].payload.text, 'hello world')
  assert.equal(state.lastSeq, 4)
})
test('history pagination preserves order and merges work steps', () => {
  const state = timeline()
  applyEvents(state, [event(5, 'item.completed', { kind: 'tool_result', output: 'ok' }, 'b')])
  applyEvents(state, [
    event(1, 'user.message', { text: 'hello', clientMsgId: 'm' }),
    event(3, 'item.completed', { kind: 'tool_call', name: 'Read' }, 'a'),
  ])
  assert.deepEqual(
    state.items.map((item) => item.seq),
    [1, 3, 5],
  )
  assert.deepEqual(
    groupItems(state.items).map((group) => group.items.length),
    [1, 2],
  )
})
test('approval resolution updates its original card and run stop clears the running target', () => {
  const state = timeline()
  applyEvents(state, [
    event(1, 'run.started', { state: 'running' }),
    event(2, 'approval.request', { approvalId: 'a', kind: 'question' }),
    event(3, 'approval.resolved', { approvalId: 'a', decision: { behavior: 'allow' } }),
    event(4, 'run.finished', { state: 'interrupted' }),
  ])
  assert.equal(state.items.length, 1)
  assert.equal(state.items[0].complete, true)
  assert.equal(state.runId, undefined)
  assert.equal(state.state, 'interrupted')
})
test('older history cannot reset a newer run or hide an already resolved approval', () => {
  const state = timeline()
  applyEvents(state, [
    event(10, 'run.started', { state: 'running' }),
    event(11, 'approval.resolved', { approvalId: 'a', decision: { behavior: 'deny' } }),
  ])
  applyEvents(state, [
    event(1, 'run.started', { state: 'running' }),
    event(2, 'approval.request', { approvalId: 'a', kind: 'approval' }),
    event(3, 'run.finished', { state: 'completed' }),
  ])
  assert.equal(state.state, 'running')
  assert.equal(state.items[0].complete, true)
  assert.deepEqual(state.items[0].payload.decision, { behavior: 'deny' })
})
test('active run snapshots restore stop target even when the cursor is already current', () => {
  const state = timeline()
  applyEvents(state, [
    event(30, 'item.started', { kind: 'text', text: 'current', snapshot: true }, 'i'),
    event(30, 'run.state', { state: 'awaiting_background', snapshot: true }),
  ])
  assert.equal(state.runId, 'r')
  assert.equal(state.state, 'awaiting_background')
  assert.equal(state.items[0].payload.text, 'current')
})

test('resolved large approvals retain the blob cursor of the original request', () => {
  const state = timeline()
  applyEvents(state, [
    event(10, 'approval.request', { approvalId: 'a', tool: 'Write', blob: true, truncated: true }),
  ])
  applyEvents(state, [
    event(12, 'approval.resolved', { approvalId: 'a', decision: { behavior: 'deny' } }),
  ])
  assert.equal(state.items[0].lastSeq, 12)
  assert.equal(state.items[0].blobSeq, 10)
  assert.equal(state.items[0].complete, true)
  assert.equal(state.items[0].payload.blob, true)
})

test('undelivered input updates its original user message across reversed history pages', () => {
  const state = timeline()
  const notice = event(
    3,
    'item.completed',
    { kind: 'user_input', delivery: 'undelivered', clientMsgId: 'm', text: 'Queued' },
    'input_m',
  )
  applyEvents(state, [notice])
  applyEvents(state, [event(2, 'user.message', { clientMsgId: 'm', text: 'Queued', append: true })])
  applyEvents(state, [notice])
  assert.equal(state.items.length, 1)
  assert.equal(state.items[0].type, 'user.message')
  assert.equal(state.items[0].payload.delivery, 'undelivered')
  assert.equal(state.items[0].payload.text, 'Queued')
})
