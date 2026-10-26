# 任务 04：M2 审查修复与 M3 本机执行

本轮采用 A-01（真实会话归档）和 A-02（Codex 本机第 1 档沙箱）。没有登录 Claude/Codex、读取账号凭据或发送真实模型请求。参考工程只用于行为核对，没有复制实现或修改参考工程。

## M2 审查逐条记录

| 问题 | 修复 | 回归依据 |
| --- | --- | --- |
| 超大审批丢失工具/输入/问题 | 完整载荷保存在 approvals；服务端审批从该表读取，事件仅传摘要与 blob 引用，questions 保持完整；卡片展开完整内容后才能批准截断请求 | `host/tests/test_m2_review.py` 超大 Write、question 与 blob；浏览器审批链路 |
| Claude 同 API 消息拆成多条导致吞块 | 按消息与块身份去重，按块 ID/类型匹配流式条目 | `test_claude_replay.py` 文本后工具、多工具、思考后文本、重复块及追加结果序列 |
| seq 回滚后重启复用 | 独立 SQLite allocator 连接提交号段；业务事务前预留；启动取 meta 与 MAX(events.seq) 的较大值 | `test_sequence_reservation.py`、接受失败回滚及重启测试 |
| Claude 后台工具孤儿 | SDK transport 使用独立 setsid 包装；停止/退出清理进程组，包含 CLI 已退出的后代 | Linux 进程组回归通过 |
| append 不持久化/不幂等/只收首个结果 | clientMsgId 幂等接受用户事件；追加失败不自动重发；Claude 持续接收到所有已接受输入的 Result | Host append 回归及双 Result 回放 |
| 探测过重阻塞 hello/锁 | 600 秒缓存、合并并发探测，hello 只返回缓存并异步刷新；锁外完成能力/原生预检 | 慢探测、并发和锁边界回归 |
| 恢复残留 item.started | 与恢复运行状态同事务补 completed(interrupted) | 重启恢复测试 |
| delta 假 blob | 未存储的 delta 不标记 blob=true | 大 delta 回归 |
| completed 持久化 snapshot | 写事件前移除 snapshot | 快照完成与重放去重回归 |
| broadcast Lagged 当断线 | 独立 Resync 分支执行 sync.since，保留正常投递状态 | Rust 真实 broadcast 溢出/继续接收回归；前端未知投递测试 |
| 订阅从不释放 | 切换时退订；subscriptionId token 防止旧清理取消同会话新订阅；选择先更新再等待，拒绝迟到选择 | Host 双 token、前端退订延迟快速切换回归 |
| Agent 名单多处硬编码 | Python 注册表创建适配器；Rust 环境遍历能力；登录与原生终端命令由 Host 提供 | 注册表回归，原生终端保留 Shell/目标测试 |
| send 未检查 available | 同时检查 installed/loggedIn/available | 不可用但已登录回归 |
| drain 超时僵尸连接 | 写任务失败/超时主动关闭 writer，记录固定原因，不记录 payload | Transport 超时关闭回归 |
| A-01 归档语义 | 真实会话归档/恢复；Fake 或官方确认原生缺失后才删镜像；删除/归档持 mutation_lock | 归档、恢复、活动运行拒绝与缺失原生记录测试 |

Claude 的消息 fixture 是依据[官方流式文档](https://code.claude.com/docs/en/agent-sdk/streaming-output)与固定 SDK 0.2.174 源码构造的协议回放，并非已登录真实模型的录制。真实模型验收仍待用户登录。

## M3 需求状态

| 需求 | 实现与边界 |
| --- | --- |
| PROJ-02 / PROJ-03 | 原生目录选择器；本机/服务器项目分组；路径和连接归属创建后固定；本机文件树可在服务器断线后使用 |
| FILE / 编辑器 | Rust 以持久归属分派本机文件或 SFTP，复用组件；项目根及祖先、家目录、系统目录受保护；校验链接解析结果、UTF-8/大小/NUL、mtime/size及写后内容。原位保存明确返回 inPlace；本机不显示远端下载入口 |
| TERM-02 | portable-pty / ConPTY，pwsh、cmd、绝对 Shell 路径；8 帧背压、累计 ack、目标隔离与清理；Agent 原生终端始终在远端 |
| ENV-07 | 随包资源路径与缺失原因检测；启动错误保留路径和系统原因；通道探测失败不伪装就绪 |
| Windows 执行端 | 固定官方 OpenSSH 10.4.0.0p1-Preview、PowerShell 7.4.22，下载 SHA256 校验；随机回环端口、每连接新主机/登录密钥、Job Object、无控制台、完整 stdin 脚本 |
| 反向通道 | register → 本机执行端 → streamlocal → ready 参数 → Python ProxyCommand → 实际脚本探测；归属校验、固定设备/会话入口，私钥不进入提示词/事件/日志 |
| 本机 Agent 工作目录 | deviceId + 规范化路径哈希生成 native/projectKey；Windows 大小写/分隔符等价；native 目录断开时保留 |
| 提示与权限 | Claude append 系统提示；Codex developerInstructions，第 1 档 workspace-write + network_access=true + 设备入口 writable_roots；状态栏展示生效配置；本机 file_change 标明服务器目录 |
| 授权与故障 | 按项目明确安全确认，断开失效；未就绪禁止新任务；sshd 退出监测撤销授权、关闭端点并回收远端入口；界面展示上线/离线/宽限期 |

本轮独立复核另修复：Windows 家目录/系统目录 canonical 路径前缀不一致、短路径/尾点/NTFS 流别名保护、缺失的其他项目阻塞当前保存、旧通道清理覆盖新通道、HostClient 并发首次连接、快速选择迟到覆盖、新本机文件树在线判断及 NUL 保存先破坏后报错。专项测试与实际界面回归均纳入验证。

## 检查结果

| 检查 | 已实际执行结果 |
| --- | --- |
| pnpm test | 28 passed |
| pnpm typecheck / format:check / build | 通过；Vite 仍有既有懒加载 chunk 大小提示 |
| protocol:check | 三端生成一致 |
| cargo fmt / clippy --all-targets -D warnings / test / build | 通过；55 单元、1 归属、2 本机文件、11 SSH 集成，共 69 passed；真实环境用例默认 ignored，另行执行 |
| Host Windows pytest | 55 passed / 6 skipped（Unix 专属与未登录真实模型契约） |
| Host Linux pytest | 59 passed / 2 skipped；最终复测 runId `3eb77e7a36bd`、退出 0；仅真实模型契约跳过 |
| 固定 Windows 资源 --check | 两组件全文件摘要通过 |
| Windows 回环执行实测 | 多行脚本、文件创建/修改、中文输出、退出码 7、后台进程结束、sshd 异常检测、临时密钥/端口回收通过 |
| ConPTY 实测 | pwsh/cmd、中文、退出码 23、调整尺寸、跨目标拒绝与清理通过 |
| 浏览器 smoke_ui / smoke_workbench / smoke_chat | 三组通过，覆盖路由/设置、文件冲突与旧目标、流式/工具/审批/提问/去重/未知投递 |

## 真实反向链路 E2E

最终 `real_device` **1/1 通过，381.82 秒**，使用 Windows → 生产 russh/Installer/HostClient → 测试服务器上的 agentdesk-test → 反向 streamlocal → Windows sshd/pwsh。实际通过项目如下：

| 检查 | 实际结果 |
| --- | --- |
| 产品部署与通道探测 | 最新 wheel/摘要经生产 Installer 部署，反向 socket 与 device-script `echo ok` 探测成功 |
| 多行脚本/文件/中文/非零码 | 专用本机目录创建并修改文件，跨行 PowerShell 完整执行，中文正确、退出码 7 保留 |
| Fake 工具流程 | Host Fake 工具实际执行服务器 ssh -F session.conf device-script；完整工具事件与结果通过 |
| Codex 第 1 档 | agentdesk-test 身份下，真实 codex sandbox + workspace-write/network_access/设备入口 writable_roots 成功到达 Windows；无登录、无模型调用、未放宽权限 |
| 断开重连 | 全 SSH 断开后更换密钥，设备入口与同一 session.conf 保持可用 |
| 离线恢复 | 离线启动的同一条命令实际等待，重连后恢复执行 |
| 默认等待超时 | 真实等待默认 60 秒后超时，并非缩短测试时间替代 |
| 默认宽限期 | 真实等待默认 300 秒后，设备运行收到 run.finished(interrupted) |
| 清理 | 本机进程/临时密钥/端口、Host keys/socket/ready 回收；native 和固定 session.conf 保留 |

**真实 WebView M3 另行通过**：实际 Tauri invoke/Channel、按项目授权、通道探测、图形审批按钮点击、device-script 工具执行、中文/exit7 结果在会话中渲染、本机文件读写、SQLite 草稿、停止、历史去重以及断开后授权失效。没有注入 IPC fixture。结果位于忽略的 `.tmp/e2e/task04-native-webview.json`。既有真实 M2 WebView 也复测通过。

部署后仅做未登录预检：Claude SDK 0.2.174 / CLI 2.1.310 与 Codex 0.164.0 握手通过，模型目录分别为 5/8 项，loggedIn=false，start_run 均 E_NOT_LOGGED_IN；loginAttempted=false、modelCalls=false。

最终只读状态：current=`0.2.0-ec8643885b48-af5b3cf1`，previous=`0.2.0-502382c0e309-b8c978f3`；已只读确认 previous 声明支持 schema 2（本轮未再次执行真实回滚）；受管 Host 有意保持运行，activeRuns=0；设备密钥目录/文件/socket 均为 0。本机随包 sshd/pwsh 残留进程为 0。脱敏结果分别保留在 `.tmp/e2e/task04-final-host-state.json`、`task04-native-preflight.json`。

测试服务器系统 OpenSSH 7.4 不支持 RemoteCommand，因此最初探测失败不计通过。Host 已能返回 E_DEVICE_SSH_VERSION 明确原因。专用用户目录 `~/.agentdesk/acceptance/ssh-client/` 构建并实测 OpenSSH 10.6p1 / LibreSSL 4.3.4，使用固定官方 Zig 0.13.0；归档 SHA256 在 `scripts/e2e_build_ssh_client.py` 固定，实际静态客户端 SHA256 为 `1671e0aae34b7c5f6888f4374c4ab9c629b6d8466df8a501d1c7419fd788b3b4`。只通过 Host 进程和单条验收命令的局部 PATH 使用，不修改系统客户端、sshd 或登录 shell。glibc 2.17 仍低于产品基线，不扩大支持范围。

图形入口首次验证还发现 Windows canonical 资源目录带 `\?\` 前缀，导致随包 sshd 无法完成启动探测；资源发现现统一去前缀并保留 UNC 语义，已用 canonical 路径实际回归并重新跑通图形链路。环境探测另覆盖了“文件存在但不可执行”的失败路径，真实安全软件拦截策略未人为修改。测试执行期间曾因正在运行的 Rust E2E 可执行文件被链接器占用而出现 LNK1104，待其结束后完整 Rust 检查已重新通过，未将中间失败记作通过。

## 与设计的差异及回滚

- 采用架构师 A-01：归档替代真实原生删除。新增数据库 migration 2（archived_at），旧 M2 Host 会拒绝读取新 schema。回滚应使用兼容 migration 2 的已验证版本，不能简单切到旧 M2 wheel，更不能覆盖业务库或丢弃新事件。
- Windows runner 用稳定 EncodedCommand 启动器一次读取完整 stdin，避免临时 runner 路径进入会话配置。命令不含密钥与端口，重连保持不变。
- 本机保存沿用保留文件权限的原位语义，可能被外部进程同时写入；mtime/size 与写后内容校验会报告冲突，不声称具备跨进程锁。
- 参考工程、用户 `.ssh`、系统 sshd 配置、Windows 服务和账号配置保持原状；未执行任何真实第三方登录。
- 本机改动：源码、忽略的随包资源与专用 `.tmp/e2e/local-project/` 样本；连接临时密钥在系统临时目录，由端点生命周期回收。关闭程序/断开后可复查对应测试端点，不删除其他进程或用户目录。
- 服务器改动：生产受管 Host 的新版本、保留 previous 与 state、稳定 devices/session-ssh/native 入口、独立验收客户端及唯一测试源目录。回滚先停止相关运行和通道，再停 Host，选择兼容 schema 的版本启动；native 与 state 保留。测试客户端只影响本轮启动命令的 PATH，去掉局部 PATH 即恢复系统客户端，但旧客户端不能执行本机模式。
- 清理只针对本次创建的 socket、keys、端点临时目录与测试进程；会话 Include 配置、native 工作目录、测试样本及客户端构建材料保留供复核，不批量删除历史测试版本。

## 登录后真实 Agent 验收清单

由用户本人先按 `USER-LOGIN.md` 登录，两种 Agent 分别执行以下四组，不使用桌面历史伪造原生上下文。

| Agent / 模式 | 必做验收 |
| --- | --- |
| Claude / 远端 | 创建与模型目录；文本/思考/多工具流式；超大 Write 审批完整查看、允许/拒绝/竞争；AskUserQuestion 单多选/自由输入；运行中追加两条输入；停止与后台工具清理；断线补取；原生恢复、缺失记录报错；归档/恢复 |
| Codex / 远端 | 创建与模型目录；文本/思考/命令/文件流式；命令和 diff 审批允许/拒绝/竞争；turn/steer；停止/断线补取；原生 thread 恢复与缺失记录；归档/恢复；保持远端模式原有权限 |
| Claude / 本机 | 以上交互在本机模式复验；确认系统提示 append 稳定；实际经 device-script 操作 Windows 专用目录；中文和非零码；原生文件工具只标注服务器目录；重连同会话配置与原生上下文不变；离线等待、恢复与宽限中断 |
| Codex / 本机 | 以上交互在本机模式复验；核对 developerInstructions 与实际第 1 档沙箱；模型自主经 device-script 读写专用目录并返回结果；审批、追加/停止、恢复与离线策略；不得自动放宽 danger-full-access |

本轮无模型沙箱/CLI预检与 Fake 工具链路不能替代这四组真实模型验收。安装包、真实通知点击、中文 IME、长时性能及 macOS/Linux 桌面验收仍按既有阶段边界另行组织。

## 提交与保留材料

代码与验收提交：

- `ffa36fa`：Host 审查修复、适配器与设备通道。
- `63ef827`：Windows 执行端、本机文件/终端、反向通道及协议。
- `9cb94af`：本机项目、授权状态、归档与大审批界面。
- `aacbc6d`：执行端实际探测、资源路径与 Windows 文件保护收口。
- `7c8f898`：大审批定位/元数据与执行目标提示。
- `42038bb`：真实 E2E、独立客户端准备与图形工具验收入口。

本文、契约、协作说明与登录清单另行形成文档提交。固定组件与测试脚本只提交源码、版本和校验值；没有提交随包二进制、测试服务器端点、密码、私钥或账号凭据。

测试样本仅位于 `.tmp/e2e/local-project/executor-smoke/`、`reverse-channel/`、`native-webview/`；服务器独立客户端目录保留源码、归档、构建日志、crypto、bin/ssh、许可证和状态记录。若需清理，先检查没有运行与通道，再按上述确切测试目录另行处理，不删除整个 `.agentdesk` 或旧任务材料。
