import test from 'node:test'
import assert from 'node:assert/strict'
import { target, unwrap, RequestGuard, sameResource } from '../src/utils/target.ts'
test('late responses cannot update a newly selected project', () => {
  const guard = new RequestGuard()
  const a = guard.begin('a', 'one')
  const b = guard.begin('b', 'two')
  assert.equal(guard.accepts(a), false)
  assert.equal(guard.accepts(b), true)
  guard.invalidate()
  assert.equal(guard.accepts(b), false)
})
test('same project reads are ordered by request sequence', () => {
  const guard = new RequestGuard()
  const first = guard.begin('a', 'one')
  const latest = guard.begin('a', 'one')
  assert.equal(guard.accepts(first), false)
  assert.equal(guard.accepts(latest), true)
})
test('resource ownership survives new requests without changing target', () => {
  const a = target('a', 'p')
  const b = target('a', 'p')
  assert.equal(sameResource(a, b), true)
  assert.notEqual(a.requestSeq, b.requestSeq)
  assert.throws(() => unwrap(a, { target: b, data: 'wrong' }))
  assert.equal(unwrap(a, { target: a, data: 'right' }), 'right')
})
