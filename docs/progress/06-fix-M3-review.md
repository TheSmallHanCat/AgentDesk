# 任务 06：M3 独立审查修复

本轮落实 A-03，保持 Vue / Rust / Python / 协议边界，无数据库迁移。未登录任何 Agent、读取账号凭据或发送模型请求。主架构师列出的十项问题均有对应回归；服务器部署风险已复现并定位，不再列为未知原因。

## 逐条修复与回归

| 问题 | 实现与验证 |
| --- | --- |
| 本机 plan 被扩大权限 | 本机 Claude/Codex 均明确拒绝 plan：会话创建、切换模式、旧会话发送/追加及适配器入口检查 `E_LOCAL_PLAN_UNSUPPORTED`。界面禁选并解释 SSH 无法保证设备文件只读。固定 Codex 原始 read-only 沙箱实际连接设备入口返回 255，未创建本机文件；常规 workspace-write 第 1 档仍通过。`test_m3_review.py` 与真实服务器 E2E 覆盖。远端 plan 保持原权限。 |
| Host 项目授权缺失 | Host 按 projectKey 记录本次控制连接授权，新增 `device.project.enable/revoke`；send/append 强制校验。至少一个项目已授权才能提交设备密钥；最后一个撤销立即删除 keys/ready。仅授权会话生成 SSH 配置与本机提示词，撤销删除该项目全部会话入口，包括上次连接保留的入口。桌面追加同样 require，并在任何 Host 请求前检查。Host 回归、服务器双项目撤销和真实 WebView 覆盖。 |
| 追加与最后 Result 竞态 | 适配器确认接收后才写 client_msgs/用户事件。拒绝返回 `E_APPEND_CLOSED`，提示用新 run.send；无错误接受记录，不取消或改写原 run。重复失败追加、数据库/事件无残留、原 run 正常完成均有回归。 |
| enable 持有全局锁 | 只在表锁内登记 preparing/提交 ready；注册、keygen、组件检查和远端探测均在锁外。close 取消令牌并等待归属清理；完成通知与代次防止旧清理覆盖新条目。回归检查其他连接状态立即返回、准备可取消、迟到完成拒绝。 |
| DefaultShell 假设 | 上游实际仍用 DefaultShell 执行 ForceCommand/外部 Subsystem，因此仅填完整路径仍不足以兼容 PowerShell。现只读查询相同的 64 位 HKLM 键，用完整随包 pwsh 路径生成对应 cmd/PowerShell 调用，PowerShell 显式保留 LASTEXITCODE；不再依赖 `%...%` 展开，不改注册表。未知自定义 shell 明确返回 `E_LOCAL_SHELL`。真实 cmd/PowerShell 进程模拟同一调用边界并验证退出 7；回环 SSH、多行、中文和清理通过。 |
| sshd 限制不彻底 | `DisableForwarding yes`、`PermitTTY no`、`PermitUserRC no`，authorized_keys 前加 restrict。设备脚本使用 stdin 和 RequestTTY no，无 TTY 依赖。真实执行端配置断言及完整反向 E2E 覆盖。 |
| Windows 路径别名和大小写 | 比较/hash 去扩展路径前缀并规范化大小写、分隔符；保存和提示词保留首次登记大小写。覆盖驱动器、UNC、不同设备、等价路径重复登记和提示路径。 |
| 目录 junction/符号链接删除 | Windows 目录型链接使用 remove_dir，只删除链接；普通目录仍走受保护的递归删除。旧实现用真实 junction 复现 E_IO / Windows error 5；修复后目标文件保留，目录符号链接在系统允许创建时走同一专项。 |
| PTY 启动和秘密缓冲 | Windows ConPTY 原生创建 shell 时挂起，加入 Job 后恢复；归属失败终止尚未执行的进程。真实首条指令创建的子进程随 Job 回收；pwsh/cmd、中文、退出码、尺寸、目标隔离通过。HostClient 对请求 JSON 字符串和序列化帧使用退出/取消均清零的缓冲，含递归回归；初始化私钥读取后的失败路径也由 Zeroizing 保护。 |
| 前端两个 Agent 写死 | 项目默认项、原生终端菜单与标签读取能力上报的 id/name/nativeTerminal；终端 kind 支持注册表标识。新增 Agent 的浏览器流程及 workspace 回归通过。 |

新 Host 回归在修复前基线复现六项失败；项目授权接口尚不存在导致相应用例也出现清理错误，未将其冒称单一故障复现。Windows junction 另外在原 fs.rs 上单独复现。修复后上述回归全部通过。

## 待确认项结论与来源

1. **SSH_TEST_ENVIRONMENT 不绕过 ACL。** 已下载并检索固定 Win32-OpenSSH `v10.4.0.0` 官方源码；变量仅由 `is_bash_test_env` 读取，调用涉及测试路径转换和 CREATE_NO_WINDOW，没有权限检查分支。因此保留它抑制子进程窗口；临时目录仍使用当前用户私有 DACL。
2. **重连忙锁确认是问题并修复。** 旧 owner 存在时原 register 必定 E_DEVICE_BUSY。服务器通过专用回环代理黑洞旧 SSH 字节，保持旧控制连接未被 Host 发现断开，立即从新连接注册同 deviceId，成功接管且代次变化。旧 owner/generation 后续清理不影响新注册。
3. **Claude 不再依赖 Result 数量等于输入数量。** 官方 SDK 是双向消息流，query 与 receive_messages 没有提供一输入一 Result 的确认契约；本轮没有模型请求，未声称录制了真实 CLI 合并回合。Host 改为排队、原生 Result 后逐条投递，只有 CLI 已空闲且无未投递输入才结束，消除了合并风险；多输入和迟到追加回放覆盖。

官方依据：[固定 DefaultShell 读取源码](https://github.com/PowerShell/openssh-portable/blob/v10.4.0.0/contrib/win32/win32compat/pwd.c)、[固定命令启动源码](https://github.com/PowerShell/openssh-portable/blob/v10.4.0.0/contrib/win32/win32compat/w32-doexec.c)、[测试环境变量源码](https://github.com/PowerShell/openssh-portable/blob/v10.4.0.0/contrib/win32/win32compat/misc.c)、[Claude 固定 SDK Client](https://github.com/anthropics/claude-agent-sdk-python/blob/v0.2.174/src/claude_agent_sdk/client.py)、[官方流式输入说明](https://code.claude.com/docs/en/agent-sdk/streaming-vs-single-mode)。完整源码仅保留在忽略的 `.tmp/review-upstream/`，未复制进产品实现。

## 安装失败根因与恢复

在测试服务器上连续复现 `install.stop` 失败：current 未切换，随后 Host 停止。根因为 HostServer.close 先 await server.wait_closed，再关闭现有控制连接；[Python 3.12 Server.wait_closed](https://docs.python.org/3.12/library/asyncio-eventloop.html#asyncio.Server.wait_closed) 会等待活动连接关闭，导致停机脚本 30 秒超时。恢复步骤可能看到还在退出的旧 PID/socket 而误认为已启动，释放控制连接后旧进程才真正退出，与任务 05 的表象一致。

修复包括：Host 先关闭连接/取消处理任务，再等待 server；桌面获得维护确认后，先释放控制连接再停机，以兼容旧 Host；STOP 同时等待 socket 与 pidfile 清除。恢复保留原 current/previous 和原运行状态。错误恢复信息合并原始诊断，清理错误不会遮盖最初阶段。新增“客户端保持打开时 SIGTERM 必须完成”的 Linux 进程回归。

所有 SSH 安装步骤标注阶段，安装脚本细分平台、目录、下载、校验/解包、Python、依赖、wheel、原生预检和链接激活。失败保留阶段、退出码与 stderr 尾部中允许的固定错误类别；未知文本替换为 `[redacted]`，原始 stderr 不落盘，缓冲有上限且清零。诊断记录和导出均覆盖假秘密扫描。

生产安装编排实跑 **10 次完整安装升级、20 次回滚**，每步检查链接及运行状态，全部通过，539.52 秒。另注入激活前/后 exit 72，两次都恢复原链接及运行中的旧版本，假秘密未进入诊断文件。入口为 `real_install_review`，结果计数在 `.tmp/e2e/task06-installer-stress.json`。这两次故意失败与根因修复前复现的 stop 失败分别记录，未计为正常部署成功。

## A-03 文档位置

- 首次启用确认：`src/i18n/zh-CN.json` / `en.json` 的 device.security；工作台提供单项目撤销。
- `README.md` 的“本机模式安全边界（A-03）”。
- `docs/M3-CONTRACT.md` 的执行通道、Host 本机模式与会话；协议 schema 和说明同步。
- 按本任务明确要求更新上级 `../docs/远程开发工作台需求设计-Claude版.md`：第 1 节增加 A-03，第 6.4、7、12 节说明同 Linux 用户边界、项目授权用途、密钥生命周期和计划模式限制。该文件位于仓库外，原稿保存在忽略的 `.tmp/review-upstream/design.before.md`，没有修改参考工程。

所有位置均说明：服务器上以这个 Linux 用户运行的任何程序，在通道开启期间都能连接本机；按项目授权用于防止误用，不是抵抗恶意程序的隔离。未把它宣传为强安全边界。

## 本机与服务器检查

| 检查 | 实际结果 |
| --- | --- |
| pnpm test | 37 passed |
| pnpm typecheck / format:check / build | 通过；仅保留已有 Vite chunk 大小提示 |
| pnpm protocol:check | 三端生成一致 |
| Rust fmt / clippy --all-targets -D warnings / build | 通过 |
| Rust 默认测试 | 76 passed；真实服务器与 Windows 专项仍按约定 ignored |
| Windows 专项逐项执行 | 4/4 passed：模拟两种 DefaultShell、首条指令子进程 Job 回收、回环 SSH、ConPTY |
| Host Windows pytest | 100 passed / 8 skipped：Unix 专属与真实模型契约按条件跳过 |
| Host Linux pytest | 106 passed / 2 skipped；最终源回归 runId `5d5b92a45251`，仅真实模型契约跳过 |
| 固定 Windows 资源校验 | OpenSSH / PowerShell 全文件摘要通过 |
| 浏览器 smoke | 新增 m3_review 及原 chat / login / ui / workbench 五组通过 |
| 真实 WebView | 真实 IPC、通道探测、图形审批、脚本中文/exit7、本机读写、草稿/停止/历史/重连通过；界面撤销后 run_send / run_append 均返回 E_DEVICE_NOT_READY / local.run |
| 最终完整反向通道 E2E | 1/1 passed，450.17 秒；真实 60 秒等待与 300 秒宽限期，含黑洞重连、plan 拒绝和双项目撤销 |
| 安装升级与回滚 | 10 次完整安装、20 次回滚及两类失败恢复通过 |
| 秘密检查 | 跟踪文件中测试端点/密码值命中 0；Host 登录敏感值、Rust SQLite/诊断导出和安装假秘密回归全部通过 |

新增 WebView 撤销检查最初未等待桌面撤销完成，此时 Host 已先行拒绝请求（E_TARGET），没有成功执行。验收改为轮询真实 device_status 确认 enabled=false，再分别调用发送和追加，均命中桌面授权检查。没有把中间失败写成通过。Cargo 全量测试曾与运行中的 WebView 争用 agentdesk.exe，退出应用后重新完整通过，未改变链接配置规避检查。

## 提交、保留材料与边界

实现按小步提交：`e76c193`（Host 权限/追加）、`b913813`（桌面通道/撤销）、`817cef8`（Windows 执行端/ConPTY/文件）、`89719fb`（安装诊断/停机/恢复）、`c6cc583`（跨连接撤销、能力和桌面检查）。`5240ce3` 补充真实验收脚本，`9f3c92a` 收口私钥失败路径和诊断类型；本文另行提交。

机器结果保存在忽略的 `.tmp/e2e/task06-installer-stress.json`、`task06-reverse-channel.json`、`task06-browser.json`、`task06-native-webview.json`，不含真实端点、密码、私钥、登录链接或授权码。专用服务器保留 current/previous、state、native 和验收样本，未删除或覆盖用户项目；本机样本仅在 `.tmp/e2e/local-project/` 与受管临时目录。注册表、系统 sshd、系统用户和用户 SSH 配置未改动。

回滚使用兼容 schema 2 且支持 A-03 项目授权协议的已验证 Host 版本；先停止相关任务/通道，再通过产品回滚。业务库和 native 目录保留，不回退到旧 M2 数据结构。真实模型的四组登录后验收、其他桌面平台和安装包边界仍见任务 04；本轮无模型调用验证不替代它们。

子任务框架固定的模型不可用，委派均未执行；实现、核查和验证由主 agent 完成，没有引用不存在的独立复核结果。

最终只读状态：current=`0.2.0-65e5a71d0338-0ec424d1`，previous=`0.2.0-9012dba2c1cd-3b39128e`；受管 Host 保持运行，activeRuns=0，设备 keys/socket/ready 均为 0，本机随包 sshd/pwsh 残留进程为 0。状态保存在 `.tmp/e2e/task06-final-host-state.json`。本轮早期挂起的测试进程与已识别的失败执行端临时目录已按确切归属清理，未批量删除历史材料。
