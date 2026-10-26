# M0 / M1 实施接口

本文件固定本阶段前后端边界，后续调整须同时更新调用方。界面走 i18n，业务错误使用稳定 code，底层 message 用于详情。所有属性使用 camelCase。

## 目标与返回

`Target = { connectionId: string, projectId?: string, sessionId?: string, requestSeq: number }`。
目标操作返回 `Reply<T> = { target: Target, data: T }`。调用方核对全部目标字段和 requestSeq；按资源保存的终端、编辑器、传输必须继续使用创建时目标。全局索引返回数组，不依赖选中目标。
错误为 `{ code: string, stage: string, message: string }`。`E_HOST_KEY_CHANGED` 拒绝连接；`E_CONFLICT` 不能自动覆盖；`E_AUTH` 不重试。

## 数据

- `Connection { id, name, host, port: number, user, authMethod: 'password'|'key'|'agent', keyPath?: string, agentKind?: 'openssh'|'pageant', proxyJump?: string, source?: string, lastUsedAt?: number }`
- `ConnectionStatus { connectionId, state: 'disconnected'|'connecting'|'awaitingTrust'|'connected'|'reconnecting'|'failed', fingerprint?: string, previousFingerprint?: string, keyType?: string, error?: string }`
- `Project { id, connectionId, name, path, description, defaultAgent: 'claude'|'codex', defaultModel, permissionMode, pinned: boolean, archived: boolean, lastOpenedAt?: number }`
- `FileEntry { name, path, kind: 'file'|'directory'|'symlink'|'other', size: number, mtime: number, permissions: number }`
- `FileDocument { path, content, mtime: number, size: number, permissions: number, lineEnding: 'lf'|'crlf'|'mixed', saveMode?: 'atomic'|'inPlace' }`
- `TerminalFrame { target, terminalId, data: number[], seq: number, exited: boolean, exitCode?: number }`。数据为字节，避免切断 UTF-8；16ms 或 32KiB 批量。最多 8 个未确认帧，前端写入后 ack。
- `Transfer { id, target, path, destination, localPath?: string, state: 'queued'|'running'|'completed'|'failed'|'cancelled', transferred: number, total: number, speed: number, error?: string }`。destination 保留下载目录供重试使用，localPath 是成功后的实际文件名。
- `Diagnostic { connectionId, stage, elapsedMs: number, success: boolean, code?: string, timestamp: number }`，不记录地址、用户名、路径和错误 payload。
- `Settings { theme: 'system'|'light'|'dark', language: 'zh-CN'|'en', closeAction: 'ask'|'tray'|'exit', scrollback: number, fontSize: number, notifications: { disconnected: boolean, completed: boolean, approval: boolean }, shortcuts: { newTerminal: string, saveFile: string, focusDock: string, closeTab: string, nextTab: string, previousTab: string, split: string } }`

## invoke

- `conn_list() -> Connection[]`; `conn_save({connection}) -> Connection`; `conn_delete({id}) -> void`（有项目或连接时拒绝）。
- `conn_import_ssh_config() -> { connections: Connection[], warnings: string[] }`（只解析不自动保存）；`conn_statuses() -> ConnectionStatus[]`。
- `conn_connect({target, credential?: string, saveSecret?: boolean | null}) -> Reply<ConnectionStatus>`；null/缺省保持凭据存储、true 保存、false 明确取消保存；无指纹时返回 awaitingTrust，不发凭据。`conn_trust_host_key({target, fingerprint}) -> Reply<null>` 只接受当前待确认指纹；然后再 connect。
- `conn_secret_status({target}) -> Reply<boolean>`，只返回当前认证类型是否已保存凭据，不返回秘密。
- `conn_disconnect({target}) -> Reply<null>`；`connection-state` 事件载荷 ConnectionStatus，只按 connectionId 更新。
- `project_list() -> Project[]`; `project_save({project, createDir: boolean}) -> Project`（远端 canonicalize，不存在返回 E_NOT_FOUND；用户确认后 createDir=true）；`project_archive({id, archived}) -> void`。
- `dir_complete({target, path}) -> Reply<string[]>`; `dir_recent({target}) -> Reply<string[]>`。
- `term_open({target, path, kind: 'shell'|'claude'|'codex', cols, rows, onOutput: Channel<TerminalFrame>}) -> Reply<{terminalId: string}>`；`term_write({target, terminalId, data: string})` / `term_resize({target, terminalId, cols, rows})` / `term_close({target, terminalId})` 均 Reply<null>。
- `term_ack({target, terminalId, seq}) -> void`，累积释放对应资源 seq ≤ N 的已发送帧额度，重复确认不增加额度；前端失败时合并重试。`term_reset()` 在前端初始化时关闭旧页面遗留 PTY 并释放信用。PTY 与 exec 必须先得到服务端确认才报告打开成功。
- `fs_list({target, path, showHidden}) -> Reply<FileEntry[]>`; `fs_stat({target,path}) -> Reply<FileEntry>`；`fs_read({target,path}) -> Reply<FileDocument>`。
- `fs_write({target,path,content,expectedMtime,expectedSize,force}) -> Reply<FileDocument>`；`fs_create({target,path})` / `fs_mkdir({target,path})` / `fs_rename({target,path,newPath})` / `fs_delete({target,path})` 均 Reply<null>。删除由 UI 确认完整路径；后端拒绝任一已登记项目根及祖先、远端用户家目录及祖先、系统一级目录，并检查父路径符号链接。
- `transfer_download({target,path,destination}) -> Reply<{transferId: string}>`（destination 为目录，后端自动避让重名）；`transfer_list() -> Transfer[]`；`transfer_cancel({target,transferId}) -> Reply<null>`；`transfer-state` 事件载荷 Transfer。重试创建新任务。
- `editor_save_local({content}) -> string | null`（Rust 原生保存对话框获得路径，前端不能指定路径；取消返回 null，重名自动编号）。
- `settings_get() -> Settings`; `settings_set({settings}) -> void`。
- `diag_list() -> Diagnostic[]`; `diag_export() -> string | null`（Rust 原生保存对话框获得路径，前端不能指定路径；仅导出脱敏诊断 JSON，取消返回 null，重名自动编号）。
- `sys_hide() -> void`; `sys_exit() -> void`（前端展示未保存编辑器、终端和传输，确认后调用；后端等待传输清理、关闭 PTY、断开 SSH 后退出）；`sys_notify({kind: 'disconnected'|'completed'|'approval', title, body, projectId?: string}) -> void`。
- 后端收到系统关闭/托盘退出请求，emit `app-close-requested`，载荷 `{ forceExit: boolean }`；由前端执行同一确认流程。通知点击 emit `notification-open`，载荷 `{ projectId?: string }`，并恢复窗口。

## 阶段约束

只建 Python 包和 hello 类型，不提供 Host 业务。M1 项目索引暂存桌面 SQLite；`sessionId` 应为空。未实现页面显示有意义的空状态，无假按钮。浏览器开发模式显示桌面能力说明，不伪造连接成功。

已有项目索引的连接不允许修改 host/port/user；应新建连接，避免把已有工作目标隐式指向另一台服务器。项目创建后目录和连接归属固定，显示属性可以修改。默认审批仅保存 default/plan，不启用高权限绕过。

## 任务 02 修正

目录下载通过归属进程组内的 `tar -czf -` 直接流式传输，不创建远端临时包。成功要求退出码为 0；60 秒仅约束数据空闲，取消/失败会显式关闭并检查归属进程组。断开连接等待传输清理完成。

保存前检查硬链接、父目录写权限、属主/组、原文件 ACL 和父目录默认 ACL；只有可安全替换的普通文件使用临时文件 + rename。其余采用原 inode 截断写入，写前完整读取、写后完整读回校验，返回 `saveMode=inPlace` 后由界面提示非原子写入的中断风险。特殊权限位文件返回 E_FILE_MODE，force 不绕过。保护范围包含相同 host/port 下其他连接与用户的已登记项目。mtime/size 仍是乐观检查，不构成远端文件锁。

所有非交互脚本经 `exec /bin/sh -s` 与 stdin 执行；交互终端继续运行远端用户 shell。
