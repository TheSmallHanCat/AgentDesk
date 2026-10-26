import type { HostEvent } from '../protocol/generated.ts'

export type Delivery = 'sending' | 'delivered' | 'unknown' | 'failed'
export interface ChatItem {
  id: string
  seq: number
  lastSeq: number
  blobSeq?: number
  runId?: string
  type: string
  payload: Record<string, unknown>
  complete: boolean
}
export interface ChatTimeline {
  items: ChatItem[]
  lastSeq: number
  runId?: string
  state: string
  stateSeq: number
  resolutions: Record<string, { seq: number; payload: Record<string, unknown> }>
  deviceState?: Record<string, unknown>
  deviceSeq?: number
  usage?: Record<string, unknown>
  seen: Set<number>
}
export function timeline(): ChatTimeline {
  return { items: [], lastSeq: 0, state: 'idle', stateSeq: 0, resolutions: {}, seen: new Set() }
}
export function applyEvents(state: ChatTimeline, events: HostEvent[]) {
  for (const event of events) {
    const snapshot = event.payload.snapshot === true
    if (!snapshot && state.seen.has(event.seq)) continue
    state.seen.add(event.seq)
    state.lastSeq = Math.max(state.lastSeq, event.seq)
    const payload = event.payload as Record<string, unknown>
    if (event.type === 'run.started' || event.type === 'run.state') {
      if (event.seq >= state.stateSeq) {
        state.stateSeq = event.seq
        state.runId = event.runId
        state.state = String(payload.state ?? 'running')
      }
      continue
    }
    if (event.type === 'run.finished') {
      if (event.seq >= state.stateSeq) {
        state.stateSeq = event.seq
        state.state = String(payload.state ?? 'completed')
        state.runId = undefined
      }
      continue
    }
    if (event.type === 'usage') {
      state.usage = payload
      continue
    }
    if (event.type === 'device.state' || event.type === 'tunnel.state') {
      if (event.seq >= (state.deviceSeq ?? 0)) {
        state.deviceSeq = event.seq
        state.deviceState = payload
      }
      continue
    }
    if (event.type === 'session.updated') continue
    if (event.type === 'approval.resolved') {
      state.resolutions[String(payload.approvalId)] = { seq: event.seq, payload }
      const item = state.items.find((item) => item.id === 'approval:' + payload.approvalId)
      if (item && event.seq > item.lastSeq) {
        item.payload = { ...item.payload, decision: payload.decision, deviceId: payload.deviceId }
        item.lastSeq = event.seq
        item.complete = true
      }
      continue
    }
    const id =
      event.itemId ??
      (event.type === 'user.message'
        ? 'message:' + payload.clientMsgId
        : event.type === 'approval.request'
          ? 'approval:' + payload.approvalId
          : 'event:' + event.seq)
    let item = state.items.find((value) => value.id === id)
    if (item && event.seq < item.lastSeq) continue
    if (!item) {
      item = {
        id,
        seq: event.seq,
        lastSeq: event.seq,
        runId: event.runId,
        type: event.type,
        payload: {},
        complete: false,
      }
      state.items.push(item)
    }
    item.lastSeq = event.seq
    if (payload.blob) item.blobSeq = event.seq
    item.type = event.type === 'item.delta' ? 'item.started' : event.type
    if (event.type === 'item.delta' && !snapshot) {
      item.payload = {
        ...item.payload,
        kind: payload.kind,
        text: String(item.payload.text ?? '') + String(payload.delta ?? ''),
      }
    } else {
      item.payload = { ...payload }
    }
    item.complete = event.type === 'item.completed' || event.type === 'user.message'
    const resolution = state.resolutions[String(payload.approvalId)]
    if (event.type === 'approval.request' && resolution) {
      item.payload = {
        ...item.payload,
        decision: resolution.payload.decision,
        deviceId: resolution.payload.deviceId,
      }
      item.complete = true
      item.lastSeq = resolution.seq
    }
  }
  state.items.sort((a, b) => a.seq - b.seq)
}
export interface ChatGroup {
  id: string
  type: 'steps' | 'item'
  items: ChatItem[]
}
export function groupItems(items: ChatItem[]): ChatGroup[] {
  const groups: ChatGroup[] = []
  for (const item of items) {
    const tool = ['tool_call', 'tool_result', 'file_change'].includes(String(item.payload.kind))
    const previous = groups.at(-1)
    if (tool && previous?.type === 'steps' && previous.items[0].runId === item.runId)
      previous.items.push(item)
    else groups.push({ id: item.id, type: tool ? 'steps' : 'item', items: [item] })
  }
  return groups
}
export function isRunning(state: string) {
  return ['running', 'awaiting_approval', 'awaiting_background', 'stopping'].includes(state)
}
