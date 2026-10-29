# 任务 07：M3 第二轮独立审查修复

任务清单已落实并分批提交。保持 Vue / Rust / Python / 协议边界，无数据库迁移或权限架构变更。先审阅并继续完成上次遗留的两个 Host 修改和 `test_m3_review2.py`；没有丢弃半成品。未登录任何 Agent、读取 Agent 账号凭据或发送模型请求。

## 逐条修复与回归

| 审查问题                         | 修复                                                                                                                                                                                                                                              | 回归证据                                                                                                                                                                                                       |
| -------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 撤销项目后活动 run 继续执行      | Host 授权改按 projectId 保存；撤销先校验全部 session ID，再删该项目 conf，调用 `_device_interrupt` 停止全部相关 run，结束事件包含“本机授权已撤销”。其他项目保留授权与设备密钥。                                                                   | `test_revoke_stops_only_project_runs`、`test_revoke_validates_all_ids_before_mutation`；服务器真实双项目运行中撤销 A，A 中断、conf 消失，B 活动 run 保留且可继续实际执行命令。                                 |
| offline / 新 register 恢复旧入口 | 离线、接管注册及 Host 启动恢复均清理设备全部 session conf。ready 后仅为新授权集合重建入口，并中断集合外活动 run。旧 owner / generation 清理仍不能影响新注册。                                                                                     | `test_offline_and_reconnect_only_b_never_restore_a`、原迟到 probe 测试；服务器宽限期内仅启用 B 后，A conf 不恢复、A run 中断，B 的实际脚本成功。                                                               |
| Claude stop 后继续投递队列       | stopping 时拒绝 append；Result 分支锁内关闭追加并退出，不查询队列；连接完成后也先检查 stopping。Codex 的 steer 在同一输入锁内检查 stopping、结束状态。                                                                                            | Claude 追加后立即 stop 的 SDK 回放断言 queries 仅含首条；`test_startup_append_is_retryable_and_codex_stop_blocks_steer` 断言停止后没有 steer。服务器 Fake 的睡眠后追加消费步骤被 stop 取消，追加内容没有执行。 |
| 异常结束时已确认输入静默丢失     | Claude finally 首先锁定并关闭 acceptingInput，再 disconnect。Host 记录每条追加的接收及投递状态，watcher 与追加落库串行，结束时为每条已确认但未投递输入发 `item.completed` / `delivery:undelivered`。前端按 clientMsgId 合并到原用户消息。         | stop / 错误 Result / CLI 退出 / emit 异常四种回放；disconnect 被暂停期间追加被拒绝，未送达事件恰好一条。前端逆序历史和浏览器均显示“未送达，可重新发送”，不生成重复消息。                                       |
| 同路径重复项目授权分裂           | 确认桌面创建入口与 SQLite 均允许不同 ID 使用同一目录。保留该行为，Host 改以 projectId 独立授权，projectKey 仍只决定 nativeRoot。Host 发独立设备状态通知，Rust 缓存并监听 closed，清除本地授权和归属执行端；撤销回复 closed 也显式关闭通道。       | Rust 本机文件测试实际保存两个同路径项目；Host `test_same_path_projects_keep_independent_grants` 证明撤销 P1 后 P2 可发送；无会话订阅的控制连接也收到 closed，HostClient 回归核对通知转发和缓存。               |
| install_error 覆盖恢复字段       | 有 details 时仅补 installStage，保留 recoveryCode、recoveryStage、hint 及已有字段。                                                                                                                                                               | probe 失败加恢复失败回归在修改前复现 recoveryCode 为 null，修复后字段全部保留。                                                                                                                                |
| STOP / 恢复失败无法撤销排水      | 失败后重新从 HostPool 获取客户端，再发 cancelShutdown 并检查 cancelled；失败记录固定 code/stage，错误 details 带 shutdownCancelCode、shutdownCancelStage 和 60 秒租约提示。嵌套恢复错误继续保留这些信息。移除对已断开旧客户端的无效请求。         | 本机真实 SSH 协议夹具分别注入 STOP、恢复脚本失败，断言建立新控制通道、随后 run.send 可用；另注入取消失败，核对返回详情和诊断记录。                                                                             |
| 项目授权没有执行路径限制         | 不改变 A-03；首次提示、README 安全章节和 M3 契约均明确：启用任一项目，该会话即可用当前 Windows 用户身份访问整台电脑中该用户有权访问的资源；项目路径只是工作目录约定，不是访问限制。                                                               | 浏览器核对首次提示包含整机访问与路径约定；文档位置见下文。                                                                                                                                                     |
| ForceCommand 智能单引号          | 采用审查允许的拒绝方案：U+2018–U+201B 返回 E_LOCAL_PATH，说明不支持排版单引号；ASCII 单引号继续双写转义，避免改变合法路径字符含义。                                                                                                               | 四种 Unicode 字符及 ASCII 引号的 Rust 回归；真实 cmd / PowerShell DefaultShell 专项通过。                                                                                                                      |
| 启动追加窗口 / 前端错误处理      | 未就绪返回 E_APPEND_NOT_READY；前端最多短暂重试同一消息身份。E_APPEND_CLOSED 后用相同文本、新 clientMsgId 发 run.send，必要时等待旧 run 释放。始终绑定最初 runId；停止、断线或切换会话取消待重试。补齐追加关闭、本机 plan、设备未授权的双语文案。 | 前端启动重试、换身份发送、停止取消、原 run 归属和翻译测试；原“投递未知不重发”测试继续通过。                                                                                                                    |
| 追加超时被误当作明确拒绝         | 复查自动重发路径时发现 Codex steer 超时等可能已写出；改返回 E_DELIVERY_UNKNOWN，保留未知状态，禁止自动改成新发送。                                                                                                                                | 新回归先复现 E_APPEND_CLOSED，修复后 Windows / Linux 全量测试通过；无 client_msgs 假接受记录，原 run 不被取消。                                                                                                |
| zeroize 缺口                     | HostClient 先用计数 writer 得到完整 JSON 加换行大小，预分配 Zeroizing 缓冲，再 to_writer；不再通过 to_vec 后 push。执行端私钥通过 mem::take 转移到凭据和请求 Value，再由 RequestBody 清零，避免 to_string 与 serde 中间私钥副本。                 | 覆盖容量边界、转义/中文序列化、超限时无 I/O、递归清零与部分写失败；私钥转移回归核对 String 数据指针保持不变且原持有者为空。                                                                                    |
| async shutdown 阻塞              | Job、进程句柄、临时目录提取到归属清理对象；stop、启动失败清理及异步上下文 Drop 的等待/删除重试均放 spawn_blocking，凭据先清零。                                                                                                                   | 单线程 Tokio 测试持有不可删除文件，同时运行定时器释放句柄，确认清理没有阻塞运行时；真实执行端、Job 子进程和目录回收通过。                                                                                      |
| 默认 Agent 存为空字符串          | 未选择 defaultAgent 时禁用保存，保存方法也提前返回，仍使用 Host 能力提供的标识。                                                                                                                                                                  | 浏览器暂停能力响应，填写其他字段后保存仍禁用；选择 Registered Agent 后保存的值正确。                                                                                                                           |

## 推测项结论

- **启用 / 撤销并发：确认存在状态交错窗口，已修。** Rust 使用每连接操作锁串行化启用/撤销，不跨网络持有全局条目锁，不阻塞其他连接；close 继续直接取消准备。Host 在已有 mutation_lock 下串行修改授权。两侧分别验证同连接串行、其他连接不受阻、撤销中到来的新启用不会丢失。
- **设备接管 DoS：同 Linux 用户可以声明相同 deviceId 并接管，属于 A-03 已接受的信任范围。** deviceId 不是身份凭据，通道仍受 SSH Linux 用户权限保护。本轮不引入设备身份认证架构；真实黑洞旧控制连接后立即接管的回归通过，旧 owner/generation 不得清理新设备。
- **`\.\` / `\??\` 前缀：确认是应拒绝的设备命名空间别名，已低成本修复。** Host 规范化前拒绝；Rust 本机项目创建与文件入口同步拒绝。合法 `\?\` 驱动器 / UNC 扩展路径继续按原规则处理。两端补测试。

## 实际验证

| 检查                                                            | 最终结果                                                                                                                                           |
| --------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| pnpm test                                                       | 42 passed，0 failed。                                                                                                                              |
| pnpm typecheck / build / format:check                           | 全部退出 0。build 保留既有大于 500 kB 的 chunk 体积提示。                                                                                          |
| pnpm protocol:check                                             | 三端 3 个生成输出一致。                                                                                                                            |
| cargo fmt --check / clippy --all-targets -- -D warnings / build | 全部退出 0。                                                                                                                                       |
| cargo test --locked                                             | 85 passed，0 failed，12 个外部环境专项默认 ignored；专项结果单列。                                                                                 |
| Windows Host pytest                                             | 114 passed，8 skipped。                                                                                                                            |
| 测试服务器 Linux Host pytest                                    | 120 passed，2 skipped；真实模型契约继续跳过。                                                                                                      |
| cargo test --lib -- --ignored --nocapture                       | 4 passed：真实本机执行端、ConPTY、两种 DefaultShell、首条指令子进程 Job 归属。                                                                     |
| 隔离服务器反向通道 E2E                                          | 全套通过，15 个固定 PASS 标记；真实等待 60 秒和宽限期 300 秒均执行。原始 Codex read-only 探测退出 255、localWrite=false；第 1 档实际设备操作成功。 |
| 浏览器 smoke_m3_review                                          | 安全提示、撤销、plan 禁选、能力终端、未送达显示、默认 Agent 校验全部通过；使用 IPC 夹具。                                                          |
| diff / 秘密与资源核对                                           | git diff --check 通过；任务 diff / 提交扫描未发现测试服务器受保护值。5 次隔离 Host 的 pid、密钥、ready、session conf、socket 均归零。              |

可重复入口见 `scripts/e2e/README.md` 的任务 07 一节。反向验收使用当前源码的隔离 Fake-only Host，路径为专用测试目录中的唯一子目录，未升级、停止或改写现有已安装 Host；继续使用生产 Rust SSH / HostClient / 执行端。Linux pytest 也上传到专用目录运行。最终本机完整检查将 TEMP/TMP 固定到 `.tmp/t/`；测试修正了跨盘临时目录假设，Unix socket 用短路径夹具。

复现记录：干净 `822b61a` 副本上的九项 Host 回归全部失败；续跑半成品首次独立运行是 8 失败 / 1 通过；前端新增前三项回归在修复前失败；恢复字段与追加超时分别有修改前失败记录。服务器首轮失败曾由新增 B 项目未登记桌面归属导致 E_TARGET，补齐真实归属后全套通过。Linux 原快照测试仅依赖 50ms 睡眠，在负载下只收到一条增量；改为等待第二条增量并暂停该边界后完整重跑通过。未将这些中间失败记为通过。

安全结果只保存在被忽略的 `.tmp/`，包括 `task07-m3-review2.json`、`task07-linux-host.json`、`task07-resource-audit.json` 和本机摘要日志。没有把端点、密码、私钥或 Agent 输出写入报告、提交或截图。

## 文档、提交与边界

- 首次提示：`src/i18n/zh-CN.json`、`src/i18n/en.json` 的 `device.security`。
- 安全说明：`README.md` 的“本机模式安全边界（A-03）”。
- 行为契约：`docs/M3-CONTRACT.md` 的执行通道、Host 本机模式与会话段落。
- 验收入口：`scripts/e2e/README.md` 的任务 07 一节；新增 `scripts/e2e/e2e_m3_review2.py`。
- 实现提交：`66adfdc`（Host 撤销和投递）、`bd765a8`（前端）、`9d68f3c`（Rust 核心）、`2445838`（追加超时防重发）；验收脚本与本报告另作提交。

SDK 调用依据为[固定 Claude SDK 的 query / interrupt / disconnect 源码](https://github.com/anthropics/claude-agent-sdk-python/blob/v0.2.174/src/claude_agent_sdk/client.py)，只核对既有官方调用，不替换 SDK 或 CLI，也不发模型请求。

`agentdesk-run` worktree 仍固定在 `822b61a`，本轮没有修改其文件、结束其进程或触发登录。未启动可能干扰既有单实例窗口的真实 WebView 验收；前端由浏览器实测，通道由生产 Rust 链路和真实 Windows 组件实测。未重跑十轮安装压力矩阵；本次安装恢复修改由本机真实 SSH 失败注入回归覆盖，没有触碰现有服务器安装。真实 Agent 的登录后行为、安装包发布仍属于原有独立验收范围。

A-03 的整机用户权限边界没有缩小；按项目授权负责防误用。可按上述小步提交逆序回退，未变更数据库 schema；已取消的 run 不会因代码回退自动恢复。
