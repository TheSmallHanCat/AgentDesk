# 真实服务器验收

这些命令只用于已授权的测试服务器，不进入普通 CI。Rust 用例全部带 `#[ignore]`，脚本缺少环境变量时输出变量名并成功跳过；没有服务器不影响默认测试。

## 准备

本机需要仓库开发工具链、OpenSSH `ssh-keygen`、`tar`，以及 Python 依赖：

```bash
python -m pip install -r scripts/e2e/requirements.txt
```

通过进程环境提供 `AGENTDESK_E2E_HOST`、`AGENTDESK_E2E_PORT`、`AGENTDESK_E2E_ROOT_USER`、`AGENTDESK_E2E_ROOT_PASSWORD`。不把具体值放入命令示例、仓库、截图或日志。

```bash
python scripts/e2e/e2e_prepare.py probe
python scripts/e2e/e2e_prepare.py provision
python scripts/e2e/e2e_prepare.py fixtures
```

`probe` 只读检查系统、工具和有效 sshd 转发/认证策略。首次连接的公钥存入无端点信息的本机 pin，后续变化拒绝连接。`provision` 只接受新建用户，或带本套脚本归属标记的既有 `agentdesk-test`；不会接管其他同名用户。它生成专用无密码短语/加密 ed25519、RSA PEM，以及不授权的错误密钥，密码和密码短语只在被忽略的 `.tmp/e2e/`。公钥追加前去重。

`fixtures` 只在 `/home/agentdesk-test/agentdesk-e2e/` 中写入、覆盖或打包，创建约 210 MiB 不可压缩样本、权限/链接/ACL/编码用例；另在专用用户 `.bashrc` 追加一段有标记的 PATH 配置，用于检验交互 shell。重复运行会恢复样本，避免在测试中放入有价值的文件。

若服务器禁用公钥认证，普通操作自动使用专用用户密码，密钥和 agent 验收明确跳过，不修改 sshd 配置。

## 执行

```bash
python scripts/e2e/e2e_run.py
python scripts/e2e/e2e_audit.py
```

可聚焦执行 `--filter real_authentication`、`real_files`、`real_downloads`、`real_streamlocal` 或 `real_blackhole`。运行器只转发固定验收标记和受控诊断，结果保存在 `.tmp/e2e/acceptance-*.json`。Rust 直接入口为：

```bash
cargo test --locked --manifest-path src-tauri/Cargo.toml --test real_server -- --ignored --nocapture --test-threads=1
```

测试连接配置仅存在内存 SQLite；应用 known_hosts 使用系统临时目录，不写入仓库或系统 SSH 配置。黑洞测试通过本机专用代理只丢弃本次测试的字节，退出后取消全部代理任务，不设置 iptables、不改变本机或服务器全局网络。检查 45 秒检测、失败重连后的恢复、重连中停止，以及旧 PTY 和写操作不重放。

S1 验证真实 russh 正反向 streamlocal、0600 权限和本机回环 TCP echo。旧 OpenSSH 取消监听后可能留下 socket inode，测试先确认无法连接，再显式删除本次测试路径；报告单列此行为。测试所用 echo 服务和 socket 都在专用目录。

Windows agent 只在既有服务可用时添加测试密钥，结束后移除；不启动或更改服务。Pageant、原生输入法、缩放、通知、安装包及长期性能仍需要人工验证，不能由协议测试推定通过。

## 工作台交互回归

启动 `pnpm dev --host 127.0.0.1` 后，运行：

```bash
python scripts/browser/smoke_workflow.py --url http://127.0.0.1:1420
```

使用隔离 IPC 数据验证空会话创建、环境目标保持、返回会话与草稿恢复、项目默认值、能力刷新、本机权限限制、迟到创建响应及 980×680 窗口的聊天与终端布局。截图与结果写入 `test-results/workflow/`，不连接真实服务器、不完成 Agent 登录、不发送模型请求。

`host/tests/test_process_environment.py` 使用临时进程检查 SSH 连接元数据过滤、服务器配置保留、官方终端参数转义和远端运行上下文。Windows 可通过 Git Bash 验证 CLI 包装器，登录 PTY 项目只在 Unix 执行。`smoke_workflow.py` 同时核对 Agent、项目及终端位置；`smoke_login.py` 核对本机浏览器授权说明及显式打开行为。

## 隔离工作区回归

```bash
uv run --project host python -m pytest host/tests/test_database_format.py host/tests/test_isolation.py host/tests/test_isolation_image.py -q
python scripts/browser/smoke_isolation.py --url http://127.0.0.1:1420
```

Python 用例验证数据库初始化、同格式数据保留、不兼容格式只读拒绝、容器策略、真实 stdio Worker 的审批与停止、未知送达状态、登录归属、空闲回收及清理失败。浏览器用例验证隔离能力缓存、禁止宿主终端、明确选择运行方式及缺少镜像时禁止保存；均不访问真实模型服务。

`scripts/build/prepare_isolation_image.py` 默认只准备经过校验的镜像上下文。已有镜像的 Linux 实机边界检查使用 `scripts/e2e/smoke_isolation_runtime.py --image IMAGE_ID`，不登录或调用模型。具体前置与回退说明见 [隔离工作区契约](../../docs/ISOLATION-CONTRACT.md)。

## M2 Spike 与残留

第二部分结束后执行 `python spikes/e2e_spikes.py`，详见 `spikes/README.md`。它只安装到专用用户下，绝不登录或发送模型提示词。

全部测试结束后再次执行 `python scripts/e2e/e2e_audit.py`，检查测试进程、socket 与临时归档归零。样本、账号、授权密钥及独立安装按报告保留，供复核；本套脚本不自动删除账号或 `~/.agentdesk` 安装目录。回滚范围以 `docs/archive/progress/02-fix-and-e2e.md` 的清单为准。

## 任务 03：服务器补测与 M2

仅在用户已授权测试服务器系统调整时运行首条命令。它会备份 sshd、记录原 sysctl 配置/数值，重载后立即新建 root 密码连接；失败恢复。不会修改 grub、重启、开启 Windows agent 服务或登录任何 Agent。

```bash
python scripts/e2e/e2e_server_adjust.py
python scripts/e2e/e2e_run.py --filter real_authentication
python scripts/build/build_host_resources.py
python scripts/e2e/e2e_run.py --suite real_host --filter real_host
python scripts/e2e/e2e_host_tests.py
python scripts/e2e/e2e_native_preflight.py
```

所有服务器入口保留“缺环境变量时成功跳过”。`real_host` 通过产品 Rust Installer/HostClient 实际上传与校验资源、安装独立 Python/SDK/CLI，启用 Fake，新建/流式/工具/审批竞争/幂等/停止/SSH 断开存活/重连补取/无重复、取消不切版本、停止重启及 previous 双向回滚。脚本仅管理专用测试 Host，开始前拒绝已有活跃运行。它不会调用模型或登录/退出账号。

`e2e_host_tests.py` 将当前源和测试压缩上传至唯一专用目录，在独立测试环境运行 Linux pytest，覆盖 Unix socket、flock、PID、空闲退出、孤儿工具进程清理等；真实模型契约保留条件跳过。`e2e_native_preflight.py` 针对最终安装路径检查两种适配器的版本、握手、模型目录与未登录拒绝路径，认证原文不输出。

真实 Windows WebView 的 IPC 检查需要构建 debug 程序，并启动其配置的开发 URL（默认 1420）；不注入浏览器 IPC fixture，也不拍含服务器信息的截图：

```bash
pnpm dev --host 127.0.0.1 --port 1420 --strictPort
cargo build --manifest-path src-tauri/Cargo.toml
python scripts/e2e/smoke_m2_native.py
```

该脚本使用隔离桌面目录、专用公钥和已固定的主机公钥验证 SSH，经过真实 Tauri invoke/Channel、Host 与 SQLite 草稿；最终检查断开后 Host 存活。已部署 Host 的完整安装矩阵由 real_host 覆盖，WebView 脚本不重复安装。没有环境变量或缺 debug 程序时明确跳过；需要本机 Playwright 与 Chromium。

前端离线主流程与 M1 回归：

```bash
python scripts/browser/smoke_chat.py --url http://127.0.0.1:1420
AGENTDESK_UI_URL=http://127.0.0.1:1420 python scripts/browser/smoke_ui.py
AGENTDESK_UI_URL=http://127.0.0.1:1420 python scripts/browser/smoke_workbench.py
```

前端 fixture 只存在 tests/，不会进入产品构建。M2 结果、服务器改动与回滚见 `docs/archive/progress/03-M2.md`；用户本人登录见 `docs/USER-LOGIN.md`。验收后受管 Host 是有意保留的部署，不属于残留实验进程；旧版本、staging 和专用测试目录保留用于复核，不自动删除用户数据。

## 任务 04：本机执行通道

```bash
python scripts/build/build_windows_resources.py
python scripts/build/build_windows_resources.py --check
cargo test --manifest-path src-tauri/Cargo.toml executor_real_multiline_utf8_exit_and_cleanup -- --ignored --nocapture
cargo test --manifest-path src-tauri/Cargo.toml conpty_real_shells_frames_target_and_cleanup -- --ignored --nocapture
python scripts/build/build_host_resources.py
cargo test --manifest-path src-tauri/Cargo.toml --test real_device -- --ignored --nocapture --test-threads=1
python scripts/e2e/e2e_host_tests.py
python scripts/e2e/smoke_m3_native.py
```

`real_device` 使用生产 Installer、HostClient、反向 streamlocal 和 Windows 执行端，完整执行真实 60 秒离线入口等待与 300 秒宽限期；运行期间不要升级或停止该 Host。缺少测试环境时输出 SKIP，不能将此记作真实验收通过。日志只保留固定 PASS 标记，不输出端点、凭据或私钥。

测试服务器 OpenSSH 7.4 不支持 RemoteCommand。仅对该基线外服务器，可先运行 `python scripts/e2e/e2e_build_ssh_client.py`，用 `--status` 查看隔离构建状态；脚本校验官方 Zig、LibreSSL、OpenSSH 归档 SHA256，写入专用用户的 `.agentdesk/acceptance/ssh-client/`，不用 sudo、不改系统工具和登录 shell。构建完成后，给上述 Rust 用例进程提供 `AGENTDESK_E2E_SSH_CLIENT=1`，仅该次 Host 启动和验收命令使用局部 PATH。该前置不能扩大产品支持基线。

`smoke_m3_native.py` 复用任务 03 的真实 WebView 入口：新建隔离本机项目、项目授权、实际通道探测、Fake 工具经 device-script 创建文件、图形审批与结果渲染、本机文件读取/保存、断开后授权失效。它不注入 IPC fixture，不登录、不发模型请求，不截图真实服务器信息。需先启动开发 URL、构建 debug 桌面程序，并保持已部署 Host 运行。

最终结果与兼容 schema 的回滚要求见 `docs/archive/progress/04-fix-M3.md`；真实 Agent 的四组登录后验收仍待用户本人登录。

## 任务 05：软件内图形登录

```bash
python scripts/build/build_host_resources.py
cargo build --manifest-path src-tauri/Cargo.toml
pnpm dev --host 127.0.0.1 --port 1420 --strictPort
python scripts/e2e/smoke_login_native.py --deploy
python scripts/browser/smoke_login.py --url http://127.0.0.1:1420
```

`smoke_login_native.py` 使用隔离桌面目录、固定主机指纹与 `agentdesk-test`，真实点击环境页登录/取消；检查 Claude 链接/回填、Codex 设备码/链接、进程结束、凭据文件元数据不变、Host/桌面日志和 SQLite 及诊断记录无链接/代码。`--deploy` 通过产品 Installer 更新专用 Host；重跑已部署版本可省略。不会打开授权页面、完成授权、读取凭据、截图登录提示或发送模型请求。结果仅含布尔值、计数与安全错误码，保存 `.tmp/e2e/task05-native-login.json`。

离线 `smoke_login.py` 使用测试 IPC fixture 覆盖三处入口、复制、回填、成功/失败/过期、原始输出兜底、取消、切目标、断线、确认退出和浏览器 Storage 清理。真实诊断导出函数的落盘扫描另由 Rust `login_notifications_are_live_only_and_do_not_reach_diagnostics_or_sqlite` 覆盖；原生脚本检查诊断记录，不驱动系统保存对话框。

## 任务 06：M3 独立审查与安装失败恢复

```bash
python scripts/build/build_host_resources.py
cargo test --locked --manifest-path src-tauri/Cargo.toml --test real_install_review -- --ignored --nocapture --test-threads=1
# 上条完成后再运行反向通道，避免升级正在使用的 Host。
AGENTDESK_E2E_SSH_CLIENT=1 cargo test --locked --manifest-path src-tauri/Cargo.toml --test real_device -- --ignored --nocapture --test-threads=1
python scripts/e2e/e2e_host_tests.py
python scripts/browser/smoke_m3_review.py --url http://127.0.0.1:1420
python scripts/e2e/smoke_m3_native.py
```

`real_install_review` 通过环境页使用的同一安装/恢复编排执行 10 次完整安装升级、20 次回滚，并注入激活前/后失败，核对原版本链接和运行状态恢复。新诊断只保留固定阶段、退出码和 stderr 尾部的白名单错误类别，未知文本替换为 `[redacted]`。不打印原始日志或秘密。

`real_device` 增加黑洞旧控制连接后立即接管注册、本机 Claude/Codex plan 明确拒绝、原始 Codex read-only 设备入口实测、单项目撤销和最后项目密钥删除；原有 60 秒等待、300 秒宽限期与资源回收保持真实时长。`smoke_m3_native` 增加真实界面撤销后桌面 send/append 拒绝；浏览器 `smoke_m3_review` 用隔离 IPC fixture 验证安全提示、撤销按钮、plan 禁选和能力上报中的新增 Agent。

Windows 另外执行 `runner_command_works_with_cmd_and_powershell_default_shells` 和 `first_instruction_child_is_owned_by_the_job` 的 ignored 用例，均带 `--lib -- --ignored --nocapture`；分别模拟 cmd/PowerShell DefaultShell 的实际命令调用以及启动首条指令创建的子进程回收。不修改真实注册表。不要在真实 WebView 运行时执行会重新链接 `agentdesk.exe` 的 Cargo 命令。

## 任务 07：隔离 Host 与撤销/重连回归

```bash
python scripts/e2e/e2e_m3_review2.py
python scripts/e2e/e2e_host_tests.py
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib -- --ignored --nocapture
python scripts/browser/smoke_m3_review.py --url http://127.0.0.1:1437
```

`e2e_m3_review2.py` 将当前 Host 源码放入 `/home/agentdesk-test/agentdesk-e2e/m7-<id>/`，仅注册 FakeAdapter，使用独立数据库与 socket，不升级或停止已安装 Host。Rust 继续走生产 HostClient、反向 streamlocal 和随包 Windows 执行端；通过 `AGENTDESK_E2E_DEVICE_HOME` 选中本次隔离入口，该值仅接受测试目录内路径。测试服务器上已有的 Python 和专用 SSH 客户端仅作为运行前置读取。

该入口重跑任务 06 的反向通道矩阵及真实 60 秒离线等待、300 秒宽限期，新增运行中撤销单项目、仅重新授权 B 时 A 不恢复、B 仍能实际执行命令、Fake 追加后立即停止不执行追加输入。最终停止归属隔离 Host；结果仅保存固定 PASS 标记、数值与布尔值到 `.tmp/e2e/task07-m3-review2.json`，不包含端点、秘密或脚本输出。

Linux pytest 临时目录固定在测试服务器专用目录，Unix socket 用短路径夹具，避免 socket 长度限制影响生命周期测试。本机完整检查可将 `TEMP` / `TMP` 指向仓库 `.tmp/t/`；本机文件测试已兼容用户家目录与测试目录位于不同盘符。浏览器脚本新增未送达消息、默认 Agent 尚未选择时禁止保存的检查，只使用隔离 IPC 夹具。
