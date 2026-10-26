# 真实服务器验收

这些命令只用于已授权的测试服务器，不进入普通 CI。Rust 用例全部带 `#[ignore]`，脚本缺少环境变量时输出变量名并成功跳过；没有服务器不影响默认测试。

## 准备

本机需要仓库开发工具链、OpenSSH `ssh-keygen`、`tar`，以及 Python 依赖：

```bash
python -m pip install -r scripts/e2e-requirements.txt
```

通过进程环境提供 `AGENTDESK_E2E_HOST`、`AGENTDESK_E2E_PORT`、`AGENTDESK_E2E_ROOT_USER`、`AGENTDESK_E2E_ROOT_PASSWORD`。不把具体值放入命令示例、仓库、截图或日志。

```bash
python scripts/e2e_prepare.py probe
python scripts/e2e_prepare.py provision
python scripts/e2e_prepare.py fixtures
```

`probe` 只读检查系统、工具和有效 sshd 转发/认证策略。首次连接的公钥存入无端点信息的本机 pin，后续变化拒绝连接。`provision` 只接受新建用户，或带本套脚本归属标记的既有 `agentdesk-test`；不会接管其他同名用户。它生成专用无密码短语/加密 ed25519、RSA PEM，以及不授权的错误密钥，密码和密码短语只在被忽略的 `.tmp/e2e/`。公钥追加前去重。

`fixtures` 只在 `/home/agentdesk-test/agentdesk-e2e/` 中写入、覆盖或打包，创建约 210 MiB 不可压缩样本、权限/链接/ACL/编码用例；另在专用用户 `.bashrc` 追加一段有标记的 PATH 配置，用于检验交互 shell。重复运行会恢复样本，避免在测试中放入有价值的文件。

若服务器禁用公钥认证，普通操作自动使用专用用户密码，密钥和 agent 验收明确跳过，不修改 sshd 配置。

## 执行

```bash
python scripts/e2e_run.py
python scripts/e2e_audit.py
```

可聚焦执行 `--filter real_authentication`、`real_files`、`real_downloads`、`real_streamlocal` 或 `real_blackhole`。运行器只转发固定验收标记和受控诊断，结果保存在 `.tmp/e2e/acceptance-*.json`。Rust 直接入口为：

```bash
cargo test --locked --manifest-path src-tauri/Cargo.toml --test real_server -- --ignored --nocapture --test-threads=1
```

测试连接配置仅存在内存 SQLite；应用 known_hosts 使用系统临时目录，不写入仓库或系统 SSH 配置。黑洞测试通过本机专用代理只丢弃本次测试的字节，退出后取消全部代理任务，不设置 iptables、不改变本机或服务器全局网络。检查 45 秒检测、失败重连后的恢复、重连中停止，以及旧 PTY 和写操作不重放。

S1 验证真实 russh 正反向 streamlocal、0600 权限和本机回环 TCP echo。旧 OpenSSH 取消监听后可能留下 socket inode，测试先确认无法连接，再显式删除本次测试路径；报告单列此行为。测试所用 echo 服务和 socket 都在专用目录。

Windows agent 只在既有服务可用时添加测试密钥，结束后移除；不启动或更改服务。Pageant、原生输入法、缩放、通知、安装包及长期性能仍需要人工验证，不能由协议测试推定通过。

## M2 Spike 与残留

第二部分结束后执行 `python spikes/e2e_spikes.py`，详见 `spikes/README.md`。它只安装到专用用户下，绝不登录或发送模型提示词。

全部测试结束后再次执行 `python scripts/e2e_audit.py`，检查测试进程、socket 与临时归档归零。样本、账号、授权密钥及独立安装按报告保留，供复核；本套脚本不自动删除账号或 `~/.agentdesk` 安装目录。回滚范围以 `docs/progress/02-fix-and-e2e.md` 的清单为准。

## 任务 03：服务器补测与 M2

仅在用户已授权测试服务器系统调整时运行首条命令。它会备份 sshd、记录原 sysctl 配置/数值，重载后立即新建 root 密码连接；失败恢复。不会修改 grub、重启、开启 Windows agent 服务或登录任何 Agent。

```bash
python scripts/e2e_server_adjust.py
python scripts/e2e_run.py --filter real_authentication
python scripts/build_host_resources.py
python scripts/e2e_run.py --suite real_host --filter real_host
python scripts/e2e_host_tests.py
python scripts/e2e_native_preflight.py
```

所有服务器入口保留“缺环境变量时成功跳过”。`real_host` 通过产品 Rust Installer/HostClient 实际上传与校验资源、安装独立 Python/SDK/CLI，启用 Fake，新建/流式/工具/审批竞争/幂等/停止/SSH 断开存活/重连补取/无重复、取消不切版本、停止重启及 previous 双向回滚。脚本仅管理专用测试 Host，开始前拒绝已有活跃运行。它不会调用模型或登录/退出账号。

`e2e_host_tests.py` 将当前源和测试压缩上传至唯一专用目录，在独立测试环境运行 Linux pytest，覆盖 Unix socket、flock、PID、空闲退出、孤儿工具进程清理等；真实模型契约保留条件跳过。`e2e_native_preflight.py` 针对最终安装路径检查两种适配器的版本、握手、模型目录与未登录拒绝路径，认证原文不输出。

真实 Windows WebView 的 IPC 检查需要构建 debug 程序，并启动其配置的开发 URL（默认 1420）；不注入浏览器 IPC fixture，也不拍含服务器信息的截图：

```bash
pnpm dev --host 127.0.0.1 --port 1420 --strictPort
cargo build --manifest-path src-tauri/Cargo.toml
python scripts/smoke_m2_native.py
```

该脚本使用隔离桌面目录、专用公钥和已固定的主机公钥验证 SSH，经过真实 Tauri invoke/Channel、Host 与 SQLite 草稿；最终检查断开后 Host 存活。已部署 Host 的完整安装矩阵由 real_host 覆盖，WebView 脚本不重复安装。没有环境变量或缺 debug 程序时明确跳过；需要本机 Playwright 与 Chromium。

前端离线主流程与 M1 回归：

```bash
python scripts/smoke_chat.py --url http://127.0.0.1:1420
AGENTDESK_UI_URL=http://127.0.0.1:1420 python scripts/smoke_ui.py
AGENTDESK_UI_URL=http://127.0.0.1:1420 python scripts/smoke_workbench.py
```

前端 fixture 只存在 tests/，不会进入产品构建。M2 结果、服务器改动与回滚见 `docs/progress/03-M2.md`；用户本人登录见 `docs/USER-LOGIN.md`。验收后受管 Host 是有意保留的部署，不属于残留实验进程；旧版本、staging 和专用测试目录保留用于复核，不自动删除用户数据。
