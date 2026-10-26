import type { Target, Reply } from '../types.ts'
let sequence = 0
export function target(connectionId: string, projectId?: string, deviceLocal?: boolean): Target {
  return {
    connectionId,
    projectId,
    ...(deviceLocal ? { deviceLocal } : {}),
    requestSeq: ++sequence,
  }
}
export function sameResource(a: Target, b: Target) {
  return (
    a.connectionId === b.connectionId &&
    !!a.deviceLocal === !!b.deviceLocal &&
    (a.projectId ?? null) === (b.projectId ?? null) &&
    (a.sessionId ?? null) === (b.sessionId ?? null)
  )
}
export function sameTarget(a: Target, b: Target) {
  return sameResource(a, b) && a.requestSeq === b.requestSeq
}
export function unwrap<T>(request: Target, reply: Reply<T>): T {
  if (!sameTarget(request, reply.target))
    throw { code: 'E_STALE', stage: 'target', message: 'Response target mismatch' }
  return reply.data
}
export class RequestGuard {
  current?: Target
  begin(connectionId: string, projectId?: string, deviceLocal?: boolean) {
    return (this.current = target(connectionId, projectId, deviceLocal))
  }
  accepts(reply: Target) {
    return !!this.current && sameTarget(this.current, reply)
  }
  invalidate() {
    this.current = undefined
  }
}
