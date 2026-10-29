# M3 本机执行契约

本文件补充 M2 契约。任务 04 的 A-01、A-02 和任务 06 的 A-03 优先于原设计中未决的删除与权限方案。

## 项目、目标与文件

桌面 `Project.deviceLocal` 缺省为 false，归属仍绑定服务器连接；本机目录由 `project_pick_local()` 原生目录选择器选取。路径在创建后不可修改。`Target.deviceLocal` 缺省为 false，Rust 仍以数据库中项目归属决定实际路由，不能仅凭 WebView 标志切换文件系统。资源操作检查创建时的连接、项目、会话和执行位置，回复原样携带请求序号。

本机 `fs_*` 复用现有 IPC 和文件/编辑器组件，由 Rust 分派本机文件系统；远端继续 SFTP。本机路径必须在项目根内，解析父目录与链接后校验。项目根、其他已登记本机项目的根及祖先、用户家目录和系统目录受保护。UTF-8 文本上限 2 MiB；保存检查 mtime/size，原位写入保留现有权限，返回 `saveMode=inPlace`。文件修改在断线时也可使用；它与服务器执行授权分离。

本机 Shell 在 Windows 使用挂起创建、加入 Job 后恢复的原生 ConPTY，其他平台沿用 `portable-pty`，`Settings.localShell` 支持 `pwsh`、`cmd` 或绝对可执行文件路径，缺省 `pwsh`。终端 ID 为 `local-*`，后续读写/尺寸/ack/关闭保持创建时目标。输出沿用 8 帧额度、累计 ack 和 32 KiB 上限。Agent CLI 原生终端仍在服务器上，由适配器注册表提供启动命令。

## 执行通道

`device_enable({target,confirmed:true})` 只接受本机项目。界面先明确说明服务器将拥有当前 Windows 用户的命令执行权限，且不是沙箱。A-03：安全边界是服务器上的这个 Linux 用户；以该用户运行的任何程序在通道开启期间都能连接本机。按项目授权用于防止误用，不是对抗恶意程序的隔离。授权仅在桌面与 Host 内存保存，断开或通道失效即清除。

启用任一项目，即允许该会话以当前 Windows 用户身份访问整台电脑中该用户有权访问的资源。项目路径只是工作目录约定，不是执行层的路径访问限制；这与桌面文件浏览器的项目根检查是不同的边界。

1. Host `device.register` 从 hello 获取设备身份与操作系统，返回 `tunnelSocket/generation/hostKeyPinRequired`。
   随后调用 `device.project.enable({projectId})` 登记 projectId。授权按项目 ID 分别保存，同一设备同一路径的两个项目互不撤销对方的授权；projectKey 仅决定原生工作目录。只有存在已授权项目才能提交设备密钥。相同 deviceId 的新控制连接在 SSH 与 hello 身份校验后接管旧注册；旧 owner/generation 的清理不能影响新注册。新注册先删除该设备全部旧会话入口。
2. Rust 生成本次连接的两组 ed25519 密钥，启动 `127.0.0.1` 随机端口 sshd；sshd 与 pwsh 归属 Job Object。无系统服务、无系统 sshd 配置、无用户 `.ssh` 改写。
3. 生产 SSH 连接申请反向 streamlocal 转发；仅接受本连接登记的 socket，将字节对接本机回环端口。
4. `device.tunnel.ready({generation,user,privateKey,hostPublicKey,scriptCommand})` 只允许登记的控制连接调用。Host 写入 0700 设备目录、0700 `keys/` 和 0600 密钥、known_hosts、配置。私钥只经过控制面，不进入事件、提示词或日志。
5. Host 用自己的 Python Unix socket ProxyCommand 执行 `ssh -F ssh.conf device-script`，输入 `echo ok`。只有退出 0 且输出正确才进入 ready；失败返回稳定错误并撤销就绪标记和本次密钥。

状态为 unprepared/preparing/probing/ready/error/closed。`device_status({target})` 返回状态和该项目是否已授权。控制连接正常也不能绕过执行通道与项目授权检查。运行中的设备状态通过 `device.state` 事件和桌面 `device-state` 事件展示。

`device_revoke({target})` 调用 Host `device.project.revoke({projectId})`，先校验全部会话 ID，再撤销该项目授权、删除其会话 Include 配置，并沿设备宽限期中断路径停止该项目全部活动 run，`run.finished.reason` 为“本机授权已撤销”。最后一个项目撤销时立即删除 keys/ready 并关闭通道。`run.send` 和 `run.append` 在桌面和 Host 两侧都检查授权，仅授权会话生成配置和本机提示词。设备离线清除全部授权、密钥与该设备全部会话 conf；`device.offline({generation?})` 检查创建时 owner 与 generation。重连探测成功后，仅重建新授权集合的会话入口，并中断其他本机活动 run。Host 的设备 closed 通知会使桌面清除对应连接授权。

Windows sshd 禁用全部转发、TTY 和 UserRC，authorized_keys 使用 restrict。ForceCommand 使用随包 pwsh 完整路径；按只读查询的 OpenSSH DefaultShell 生成 cmd/PowerShell 均正确的调用，保留非零退出码，拒绝不支持的自定义 shell，不改注册表。连接准备在全局表锁外执行，close 可以取消准备并等待归属资源清理。

设备目录固定为 `~/.agentdesk/devices/<deviceId>/`，会话配置固定为 `~/.agentdesk/session-ssh/<sessionId>.conf`，仅含一行 Include。重连换密钥，入口和系统提示词保持不变。Windows runner 从 stdin 一次读取完整脚本，用 UTF-8 执行并保留退出码；不用 `-Command -` / `-File -`。

离线入口等待 `config.json` 的 `device.wait_timeout`，缺省 60 秒；`device.grace` 缺省 300 秒，到期中断该设备运行并发送原因。断开先取消转发，再显式回收本连接 socket、密钥和临时执行目录；不清理 `native/`。`device.offline` 与断开清理均检查控制连接归属。

## Host 本机模式与会话

`project.upsert({kind:'local',id,path,name,deviceId?})` 的设备缺省为 hello 身份，显式不匹配拒绝。Windows 路径按大小写不敏感、两种分隔符等价规范化。`projectKey=SHA256(deviceId + NUL + normalizedPath)`，工作目录为 `~/.agentdesk/native/<projectKey>/`。

Claude 使用 `system_prompt` 的 append；Codex 使用固定版本支持的 `developerInstructions`。提示只包含稳定的会话 SSH 配置路径、别名、脚本输入方法和设备平台/项目位置。文件工具事件标明作用于服务器工作目录。

Codex 本机模式固定为 `workspace-write`，`network_access=true`，`writable_roots` 包含该设备入口；不修改用户 Codex 配置，不回退到 `danger-full-access`。真实模型的行为验收在用户登录后单独运行。

本机 Claude/Codex 的会话创建、模式切换和旧会话运行均拒绝 plan，错误码 `E_LOCAL_PLAN_UNSUPPORTED`。原生服务器沙箱不能保证 SSH 中的 Windows 操作只读。

Windows projectKey 比较忽略大小写、分隔符及扩展路径前缀，存储路径与提示词保留首次登记的大小写；设备命名空间前缀 `\\.\` / `\??\` 明确拒绝。Claude 追加输入保存在 Host 队列，仅在原生 Result 后且未 stopping 时提交下一条，CLI 空闲且队列为空才结束。finally 在断开 SDK 前锁定并关闭追加窗口；停止、CLI 退出、错误 Result 或事件异常结束时，每条已确认但未投递的输入均有 `item.completed`，载荷含 `kind:user_input`、`delivery:undelivered`、`clientMsgId` 和 `text`，界面在原用户消息显示“未送达，可重新发送”。Codex 的 steer 同样在停止后拒绝。

启动尚未就绪返回 `E_APPEND_NOT_READY`，桌面短暂重试相同消息身份；关闭后返回 `E_APPEND_CLOSED`，桌面使用相同文本和新的 clientMsgId 发起 run.send。仅明确拒绝可自动重试，超时、断线或投递未知不自动重发；停止或切换目标会取消待重试输入。拒绝的追加不记录 clientMsgId 或用户事件，也不改变已完成 run。

项目 defaultAgent 和原生终端 kind 使用 Host 能力键作为 Agent 标识；显示名来自 name，原生终端入口以 nativeTerminal 判断，不再在前端固定 Agent 名单。

A-01：真实 Agent 的会话删除采用归档，`session.list({archived:true})` 查看，`session.restore` 恢复，界面说明原生记录保留。仅 Fake 或官方接口确认原生记录缺失时彻底删除镜像。`run.append` 必须提供 `clientMsgId`，与 send 分别记录幂等身份和用户事件。`sync.subscribe/unsubscribe` 可提供 `subscriptionId`，旧订阅释放不会取消同会话的新订阅。

## 依赖与验收

`scripts/build/build_windows_resources.py` 固定官方 Win32-OpenSSH、PowerShell 7 版本和 SHA256，准备到忽略的 `resources/windows/`，Windows 打包流程携带，许可证见 NOTICE。开发模式使用同一准备脚本。资源缺失、启动被拒和端口故障报告具体路径与原因。

服务器 SSH 客户端需要支持 `RemoteCommand`。测试服务器 OpenSSH 7.4 低于要求，专用新版客户端仅作为基线外验收前置，细节和实际通过结果以任务 04 报告为准。
