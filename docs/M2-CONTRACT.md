# M2 集成契约

本文件补充 `IMPLEMENTATION-CONTRACT.md`。主设计与已确认决策优先。M1 桌面项目索引保留；连接 Host 时按原 id 幂等登记远端项目，不删除、自动合并或覆盖已有不同归属项目。

## Host RPC

所有方法沿用设计 5.3 的点号命名。请求参数与返回字段均 camelCase；错误为 JSON-RPC error，`code=-32000`，`data={code,stage,hint,decision?}`。协议 JSON Schema 为唯一类型来源。hello 后才可调用业务方法；deviceId 来自 hello，审批不得由参数冒充决定设备。

- `hello` 见原协议；capabilities 的键为可扩展 Agent id，值包含 installed/available/loggedIn/version/reason/models/permissionModes/streaming/resume/approvals/stop/appendInput。
- `project.upsert({id,path,name,defaultAgent?}) -> {id,path,name,...}`；默认接收 remote 项目；M3 本机项目的设备归属见 M3-CONTRACT.md。已有 id 的路径固定。
- `session.list({projectId}) -> Session[]`；`session.create({projectId,agent,model,permissionMode,title?}) -> Session`。
- `Session={id,projectId,agent,nativeId?,title,model,permissionMode,status,createdAt,updatedAt}`；时间为 ISO8601 UTC。
- `session.rename({sessionId,title}) -> Session`；`session.delete({sessionId}) -> {archived:true}|{deleted:true}`；有运行时拒绝。A-01：真实会话默认归档，返回 archived:true；session.list({archived:true}) 查看，session.restore 恢复，不删除原生记录。仅 Fake 或官方确认原生记录缺失可返回 deleted:true 并彻底删除镜像。
- `session.history({sessionId,beforeSeq?,limit?}) -> {events:HostEvent[],hasMore:boolean,nextBeforeSeq?:number}`；返回按 seq 升序排列，beforeSeq 为排他游标。
- `run.send({sessionId,clientMsgId,text,script?}) -> {runId,clientMsgId,accepted:true}`；script 仅 FakeAdapter 接受。去重不得再次创建运行或事件。
- `run.append({sessionId,runId,text,clientMsgId}) / run.stop({sessionId,runId}) -> {accepted:true}`。
- `approval.respond({sessionId,approvalId,decision}) -> {decision}`；decision 为 `{behavior:'allow'|'deny',answers?:Record<string,string|string[]>}`。后到返回 E_ALREADY_RESOLVED 与已生效决定。
- `sync.subscribe({sessionId}) -> {subscribed:true}`；`sync.since({sessionId?,lastSeq,limit?}) -> {events:HostEvent[],snapshots:HostEvent[],currentSeq,hasMore}`。currentSeq 是已扫描水位；分页时仅前进到已返回页的末尾，不能跳过事件。
- `event.blob({sessionId,seq,offset?,limit?}) -> {text,offset,total,hasMore}`；以字节游标分块，帧始终小于 4 MiB。
- `agent.status({}) -> capabilities`；`env.probe({}) -> object`；尚未实现的 P1 方法返回 E_CAPABILITY，不提供虚假业务成功；已实现 M3 接口见补充契约。
- `login.start({agent}) -> {loginId,agent}`：Host 管理官方登录进程，每个 Agent 最多一个，归属发起请求的连接 Client。`login.respond({loginId,text}) -> {accepted:true}` 转交单行输入；`login.cancel({loginId}) -> {accepted:true}` 结束进程组并关闭 PTY/app-server。旧 `agent.login.start` 已移除。
- `login.prompt` / `login.state` 为独立 JSON-RPC 实时通知（不是 HostEvent，不分配 seq、不写 events/blob/日志、不进入 sync.since）。params 包含 loginId/agent；prompt 可带 url/deviceCode/needsInput/rawOutput，state 包含 starting/waiting/succeeded/failed/expired/cancelled 及固定安全原因码、刷新后的 capability。断线立即作废，重连重新开始。
- 能力包含 `authStatus=missing|loggedOut|loggedIn|failed|checking`、公开 `accountType`；hello 只返回已有缓存，不启动 CLI。agent.status/env.probe 在指定项目的运行环境内触发并等待探测；未指定项目时检查普通环境。CLI 退出清除缓存后重新检测，成功以实际登录状态为准。
- `agent.logout({agent}) -> {loggedOut:true,capability}` 仅显式确认后调用官方退出命令；有活动运行或登录会话时拒绝。
- `host.prepareShutdown({}) -> {prepared:true,leaseSeconds:60}` 原子进入维护状态，活跃运行时拒绝；租约内 run.send 返回 E_HOST_DRAINING。`host.cancelShutdown({}) -> {cancelled:true}` 由同设备解除。SSH 停机/升级/回滚先获取该屏障，连接丢失后租约仍有效，超时自动恢复接收。

可选的 `runtimeMode`、固定 `runtimeId`、按项目登录与能力作用域、`runtime.status` 及旧数据兼容规则见 [服务器隔离工作区契约](ISOLATION-CONTRACT.md)。

`HostEvent={seq,sessionId,runId?,ts,type,itemId?,payload}`。通知为 `{jsonrpc:'2.0',method:'event',params:HostEvent}`。`item.*` 的 payload 包含 `kind` 和 `text` 或 `delta`；工具为 `name/input/output`；审批 payload 为 `approvalId/kind/tool/input/questions`。提问结构采用 Claude AskUserQuestion 的 questions（question/header/options[{label,description}]/multiSelect）。`run.finished.payload.state` 为 completed/failed/interrupted。

审批同时发送服务端请求 `{jsonrpc:'2.0',id:'approval:'+approvalId,method:'approval.request',params:HostEvent}`；响应 `{result:{decision}}` 与 approval.respond 共用先到生效的仲裁。Rust 将两条通路归一为同一 seq 的事件并去重。sync.since 的末页还返回活动 run.state 快照，使只加载最近历史的新窗口仍能停止运行。

seq 按 1024 号段预留，进程重启跳过未使用尾号，允许间隙但绝不复用；delta 分配仅使用内存，号段用尽才写元数据。事件负载与 blob 仍只持久化非 delta。run.send 的运行、幂等记录、用户消息和 run.started 在一个 SQLite 事务中接受，提交后才推送。

## Python 适配器边界

`adapters/base.py` 定义 `AgentAdapter` Protocol、`RunContext`、`RunHandle`、`AdapterError`；不依赖服务端实现。session 为上述字典，另含 `path`（项目根）；input 为 `{text,script?}`。

- `RunContext(run_id, emit, approve, save_native)`：emit 为 async `(type,payload,item_id=None)`；approve 为 async `(kind,payload)->decision`；save_native 为 async `(native_id)->None`。
- `RunHandle(run_id,task,native={})`：task 为整个运行任务，Host 负责将其退出转换为 run.finished；adapter 不重复发送 run.started/finished。
- capabilities、start_run、append_input、stop、respond_approval、native_exists 均 async；start_run 返回 RunHandle；未支持能力抛 AdapterError。
- `FakeAdapter` 由 Host 显式配置启用；`ClaudeAdapter`、`CodexAdapter` 动态检测。所有适配器未登录时先返回 E_NOT_LOGGED_IN，不发 prompt。

## 原生进程环境

Host 启动的能力检测、认证查询、退出登录、Codex app-server、登录 PTY 及官方 Agent 原生终端均排除 `SSH_CONNECTION`、`SSH_CLIENT`、`SSH_TTY`、`SSH_ORIGINAL_COMMAND`。Claude SDK 会合并父进程环境，因此在已有 CLI 进程组包装器内、执行官方二进制之前通过 `env -u` 排除这些字段；不修改 SDK 或官方 CLI。

过滤只针对四个 SSH 连接元数据字段，不修改 Host 的全局环境，不改变服务器时区、语言、代理、认证变量、配置目录、SSH agent 或构建工具链。普通 Shell 终端保留服务器原有行为。此措施不限制同用户进程读取宿主文件、进程信息或登录记录，也不构成沙箱或账号访问保证。远端项目运行参数不注入桌面设备名、设备 ID 或本机执行指令；本机项目仍按 M3 契约显式提供执行位置与通道。

SDK 环境合并依据：[固定版本 v0.2.174 的官方子进程实现](https://github.com/anthropics/claude-agent-sdk-python/blob/v0.2.174/src/claude_agent_sdk/_internal/transport/subprocess_cli.py)。

## Desktop IPC

命令名沿用设计 11.2。所有目标方法接收 `target: Target`，返回 `Reply<T>`；会话目标必须携带 sessionId。`session_subscribe({target,lastSeq,onEvents:Channel<{target,events:HostEvent[],disconnected?:boolean}>})` 返回 `Reply<{subscriptionId}>`；`session_unsubscribe` 按创建时归属清理。subscribe 先订阅再补取；前端按 seq 和 itemId 去重，快照替换进行中内容。

`session_create({target,agent,model,permissionMode,title?})`；`session_list({target})`；`session_history({target,beforeSeq?,limit?})`；`session_rename({target,title})`；`session_delete({target})`。`run_send({target,text,clientMsgId?})` 由 Rust 缺省生成 id，前端需在发送前分配 clientMsgId，以支持状态未知；不自动重发。

`run_stop({target,runId})`、`approval_respond({target,approvalId,decision})`。`env_probe`、`env_install_plan`、`env_install_run`、`env_install_cancel`、`host_start`、`host_stop`、`host_status`；安装计划含 planId/artifacts/paths/commands，执行必须匹配服务器端保存的计划。安装取消关闭归属进程组，不能切换 current。

`session_draft_get({target})->Reply<string>` / `session_draft_set({target,text})->Reply<null>` 将草稿存入桌面 SQLite。`host_rollback({target,fakeAgent?})` 仅切回已保留的受管版本，不删除版本或数据。订阅终止最后推送 `{target,events:[],disconnected:true,closed:true}`；临时断线没有 closed，前端保留订阅 id，等待 Rust 补取。

环境信息采用 `{layers:[{name,status,detail}],agents,host:{installed,running,version?,hostId?},sameHostConnections?:string[]}`；状态为 ready/missing/failed/unsupported/unknown。不得把版本可读当作沙箱或登录已就绪。

M3 本机项目、执行通道、订阅 token 与权限补充见 `M3-CONTRACT.md`。

## D-10 图形登录 IPC

`agent_status({target}) -> Reply<capabilities>`；`agent_login_start({target,agent,onEvents:Channel<LoginFrame>}) -> Reply<{loginId,agent}>`；`agent_login_respond({target,loginId,text})`、`agent_login_cancel({target,loginId})` 返回 `Reply<{accepted:true}>`；`agent_logout({target,agent}) -> Reply<{loggedOut:true,capability}>`。

`LoginFrame={target,method:'login.prompt'|'login.state',data:LoginPrompt|LoginState,disconnected?:boolean}`。Rust 在 start 前订阅独立实时通道，后续操作严格核对创建时的全部 Target（含 requestSeq），不得通过重连恢复旧 loginId。通道丢帧视为登录失效并取消；不执行补取。关闭对话框、切目标、断线或终态清除前端链接/代码/原始输出/输入。原生 CLI 终端继续保留，但不是登录入口。
