import test from 'node:test'
import assert from 'node:assert/strict'
import { setTimeout as delay } from 'node:timers/promises'
import { TerminalAcknowledgements } from '../src/utils/terminal-ack.ts'

test('a failed final acknowledgement retries even when the credit window is full', async () => {
  const sent: number[] = []
  const errors: unknown[] = []
  const acknowledgements = new TerminalAcknowledgements(
    async (seq) => {
      sent.push(seq)
      if (sent.length === 1) throw new Error('IPC unavailable')
    },
    (error) => errors.push(error),
    1,
  )
  acknowledgements.acknowledge(8)
  await delay(25)
  acknowledgements.dispose()
  assert.deepEqual(sent, [8, 8])
  assert.equal(errors.length, 1)
})

test('cumulative acknowledgement advances to the latest rendered frame', async () => {
  const sent: number[] = []
  let resolve!: () => void
  const acknowledgements = new TerminalAcknowledgements(
    (seq) => {
      sent.push(seq)
      return sent.length === 1
        ? new Promise<void>((done) => {
            resolve = done
          })
        : Promise.resolve()
    },
    () => assert.fail('unexpected acknowledgement error'),
  )
  acknowledgements.acknowledge(1)
  acknowledgements.acknowledge(8)
  resolve()
  await delay(0)
  acknowledgements.acknowledge(4)
  await delay(0)
  acknowledgements.dispose()
  assert.deepEqual(sent, [1, 8])
})

test('disposing a terminal stops acknowledgement retries', async () => {
  let attempts = 0
  const acknowledgements = new TerminalAcknowledgements(
    async () => {
      attempts++
      throw new Error('IPC unavailable')
    },
    () => {},
    10,
  )
  acknowledgements.acknowledge(8)
  await delay(0)
  acknowledgements.dispose()
  await delay(25)
  assert.equal(attempts, 1)
})
