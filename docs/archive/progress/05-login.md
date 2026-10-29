# 任务 05：软件内图形登录

实现依据：主设计 D-10、4.4、ENV-04，以及本仓库 M2/M3 契约。桌面转交官方 CLI 的交互，不提供账号密码表单，不直接调用模型服务。本轮没有完成任何账号授权、读取或复制凭据，也没有发送模型请求。

## 逐条完成情况

| 要求          | 完成情况与实现                                                                                                                                                                                                                                              |
| ------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 自动检测      | Host socket 就绪即启动所有已注册适配器探测；hello 不阻塞，agent.status/env.probe 等待检测结果。能力新增 authStatus（missing/loggedOut/loggedIn/failed/checking）与公开 accountType，异常不会假报未安装。环境、新建会话、聊天头部同步显示并提供登录/管理入口 |
| Host 登录会话 | 实现 login.start/respond/cancel、agent.logout。Claude 使用官方 auth login 的后台 PTY；Codex 优先 app-server chatgptDeviceCode，只有明确不支持接口时才回退官方 login --device-auth。每 Agent 仅一个会话，归属创建时 Client；输入单行限长且禁止控制字符       |
| 解析与退出    | 每个适配器定义自己的链接、设备码、回填与终态规则；支持 UTF-8/ANSI/OSC 分片、有界内存。PTY 不回显输入；取消、超时、断线和 Host 关闭回收进程组/句柄。退出后清除能力缓存并重新检测，输出成功文字不能替代实际登录状态                                           |
| 解析兜底      | 无法识别完整结构时实时转交清洗后的原始输出并提供输入框；已识别链接但回填提示变化也能兜底。未完成的半截 URL 不生成浏览器按钮。输入与原始输出不写日志                                                                                                         |
| 桌面对话框    | 全局 Vue 对话框；系统浏览器由 Tauri opener 打开；链接/设备码可复制；设备码大字显示；回填输入提交后清空；启动、等待、成功、失败原因、过期、取消及断线状态均有双语文案；退出登录需要二次确认                                                                  |
| 归属与竞态    | Rust 后续操作严格校验完整 Target（含 requestSeq）和创建连接；先订阅再 start，避免早提示丢失。错误启动响应关闭 Host 连接回收未知会话。前端拒绝迟到事件；取消与完成竞争时保留最终能力，不能显示虚假成功或取消                                                 |
| 不落盘        | login.prompt/login.state 独立实时通知，不分配 seq，不进 HostEvent/events/blob/sync.since。Rust 使用独立 login_events 通道，丢帧直接取消；断线作废而不补取。关闭、切目标、断线、终态清除对话框链接/代码/原始输出/输入，不使用持久化 Storage                  |
| 移除旧方案    | 删除旧 agent.login.start 与登录终端入口；原生 CLI 终端保留。USER-LOGIN.md 改为简短开发者说明，手工方式仅用于排障；README、M2 契约和 E2E 说明同步更新                                                                                                        |

退出登录在有活动运行或登录会话时拒绝；执行官方 logout 后还会重新检测，仍登录或检测失败时返回 E_LOGOUT_VERIFY。公开能力只保留账号类型，不透传邮箱或账号对象。

## 检查结果

| 检查                                                                | 结果                                                                                                                                                              |
| ------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| pnpm test                                                           | 36 passed，含 8 个登录 Store 用例及双语键校验                                                                                                                     |
| pnpm typecheck / build / format:check                               | 通过；Vite 仅有既有 chunk 大小提示                                                                                                                                |
| pnpm protocol:check                                                 | 三端生成一致                                                                                                                                                      |
| cargo fmt --check / clippy --all-targets -D warnings / test / build | 通过；72 passed，真实环境与平台专项用例按原约定 ignored                                                                                                           |
| Host Windows pytest                                                 | 94 passed / 7 skipped；Unix 专属与真实模型契约按条件跳过                                                                                                          |
| Host Linux pytest                                                   | 99 passed / 2 skipped；真实模型契约跳过，Unix PTY 实际执行                                                                                                        |
| Playwright + IPC fixture                                            | smoke_login 与 smoke_chat 实跑通过，覆盖启动、浏览器转交/复制、回填、成功/失败/过期、设备码、原始输出、取消、切目标、断线、退出确认及三处入口；未破坏既有聊天流程 |
| 敏感信息回归                                                        | Host 实际转交测试假链接/设备码/回填码后扫描 SQLite dump/文件、日志及 sync；Rust 实际路由假提示并扫描桌面 SQLite、诊断日志和 Diagnostics.export 生成文件，均无假值 |

关键测试：host/tests/test_login.py、test_login_detection.py、test_native_adapters.py；tests/login-store.test.ts；Rust commands/login.rs、host/client.rs 中的定向测试。覆盖重复登录、跨连接拒绝、并发取消/断线、完成先于取消、失效登录刷新、进程退出后复检、设备接口超时取消与错误降级边界。

## 测试服务器实测

使用固定主机指纹、既有 AGENTDESK_E2E_* 环境变量和专用 agentdesk-test 身份，经真实 Windows WebView → Tauri invoke/Channel → russh → Unix socket → Host。没有注入 IPC fixture，没有点击授权浏览器按钮，没有提交真实授权码，没有截图登录提示。

1. 使用产品 Installer 部署当前 Host wheel 与 SHA256 清单，保留旧版本。
2. 在环境页实际点击 Claude 登录，看到授权链接、“在浏览器中打开”按钮及回填框；点击取消后内容被清除。
3. 实际点击 Codex 登录，看到设备码、授权链接和浏览器按钮；无需回填输入；点击取消后内容被清除。专用服务器 Codex 0.164.0 的设备码接口确实可用。
4. 检查 /proc，Claude 登录与 Codex 登录/app-server 进程均已结束。比较 .claude/.credentials.json 和 .codex/auth.json 的存在性、大小、mtime、inode，前后一致；没有读取凭据内容。
5. 扫描 7 个 Host 日志/SQLite/WAL 文件、隔离桌面数据文件和原生诊断记录，未发现本次授权链接或设备码。真实 Diagnostics.export 落盘扫描由上述 Rust 回归覆盖；原生脚本不驱动系统“保存”对话框。

复现入口：scripts/e2e/smoke_login_native.py --deploy；已部署版本重跑可省略 --deploy。结果只保存计数、固定标记和安全错误码，位于忽略的 .tmp/e2e/task05-native-login.json；Linux 结果为 task05-host-tests.json，最终版本状态为 task05-final-host-state.json。

安装过程中两次出现 E_EXEC/exec（首轮未提取安全错误码），current 均未切换；其中一次失败后观察到 Host 已停止。重试通过产品 Installer 完成部署、启动与全部图形流程。尚未确定最初执行失败的位置，未据此扩大修改安装架构；这项部署重试风险保留记录。

## 解析样例来源

host/tests/fixtures/login/ 中两份等待授权输出重新从官方 CLI 的真实 PTY 采集，保留原文与 ANSI/OSC，统一行尾及行尾空白，URL、state 和设备码替换为假值。采集获得提示后立即取消，未使用账号。

- Claude Agent SDK 0.2.174 所带官方 CLI 2.1.310：claude auth login 的链接及 Paste code here if prompted 提示。成功文字依据[官方认证文档](https://code.claude.com/docs/en/authentication)；失败文字另通过只读检索同版本官方二进制登录分支核对。
- Codex 0.164.0：codex login --device-auth 的链接、设备码及有效期提示；另实测 app-server 返回 verificationUrl/userCode 并立即 cancel。接口依据[官方 app-server 文档](https://learn.chatgpt.com/docs/app-server)，成功、失败和超时模板依据固定版本[登录源码](https://github.com/openai/codex/blob/rust-v0.164.0/codex-rs/cli/src/login.rs)及[设备码源码](https://github.com/openai/codex/blob/rust-v0.164.0/codex-rs/login/src/device_code_auth.rs)。

详细来源与真实性边界见 fixtures/login/SOURCES.md。Claude 过期、未来格式及不支持的设备/回填组合为明确标注的构造健壮性用例；没有把成功授权或过期等待冒称真实录制。

## 已知限制与回退

- 按验收边界不完成真实授权，因此真实成功、账号类型和退出登录在本轮使用官方状态/协议 fixture 验证；真实服务器验证的是提示转交与取消，不宣称登录后模型验收通过。
- 官方 CLI 输出可能变化；有原始输出兜底，仍应在更新固定 CLI 版本时重录解析样例。登录最长 15 分钟，失效后重新开始。
- 本轮验证 Windows 桌面和 Linux Host；macOS/Linux 桌面、Windows Host PTY 不在本轮设备验收范围。Unix PTY 测试在 Linux 补测通过。
- 新桌面需配套本轮 Host。协议无数据库迁移，原生凭据由 CLI 管理，当前任务不删除任何账号/数据。回退按既有 Host previous/current 和 Git 小步提交执行；旧版本不具有图形登录能力。
- 没有新增需决策的架构问题；沿用现有三端边界及 D-10，因此未新增 QUESTIONS 条目。

最终只读状态：current=`0.2.0-53c1faf64585-7e68a5eb`，previous=`0.2.0-a21259de574f-3e95da17`，Host 运行中；专用部署按既有约定保留。Linux 最终复测 runId=`91c776e0f4d8`，99 passed / 2 skipped。
