export interface Target {
  deviceLocal?: boolean
  connectionId: string
  projectId?: string
  sessionId?: string
  requestSeq: number
}
export interface Reply<T> {
  target: Target
  data: T
}
export interface AppError {
  code: string
  stage: string
  message: string
  details?: { hint?: string; decision?: unknown; [key: string]: unknown }
}
export interface Connection {
  id: string
  name: string
  host: string
  port: number
  user: string
  authMethod: 'password' | 'key' | 'agent'
  keyPath?: string
  agentKind?: 'openssh' | 'pageant'
  proxyJump?: string
  source?: string
  lastUsedAt?: number
}
export interface ConnectionStatus {
  connectionId: string
  state: 'disconnected' | 'connecting' | 'awaitingTrust' | 'connected' | 'reconnecting' | 'failed'
  fingerprint?: string
  previousFingerprint?: string
  keyType?: string
  error?: string
}
export interface Project {
  runtimeMode?: 'direct' | 'isolated'
  deviceLocal?: boolean
  id: string
  connectionId: string
  name: string
  path: string
  description: string
  defaultAgent: string
  defaultModel: string
  permissionMode: string
  pinned: boolean
  archived: boolean
  lastOpenedAt?: number
}
export interface FileEntry {
  name: string
  path: string
  kind: 'file' | 'directory' | 'symlink' | 'other'
  size: number
  mtime: number
  permissions: number
}
export interface FileDocument {
  path: string
  content: string
  mtime: number
  size: number
  permissions: number
  lineEnding: 'lf' | 'crlf' | 'mixed'
  saveMode?: 'atomic' | 'inPlace'
}
export interface TerminalFrame {
  target: Target
  terminalId: string
  data: number[]
  seq: number
  exited: boolean
  exitCode?: number
}
export interface Transfer {
  id: string
  target: Target
  path: string
  destination: string
  localPath?: string
  state: 'queued' | 'running' | 'completed' | 'failed' | 'cancelled'
  transferred: number
  total: number
  speed: number
  error?: string
}
export interface Diagnostic {
  connectionId: string
  stage: string
  elapsedMs: number
  success: boolean
  code?: string
  timestamp: number
  exitCode?: number
  stderrTail?: string
}
export interface Settings {
  localShell: string
  theme: 'system' | 'light' | 'dark'
  language: 'zh-CN' | 'en'
  closeAction: 'ask' | 'tray' | 'exit'
  scrollback: number
  fontSize: number
  notifications: { disconnected: boolean; completed: boolean; approval: boolean }
  shortcuts: {
    newTerminal: string
    saveFile: string
    focusDock: string
    closeTab: string
    nextTab: string
    previousTab: string
    split: string
  }
}
