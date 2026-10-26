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
