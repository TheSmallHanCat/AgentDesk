# M2 前置技术验证

本目录只放独立实验脚本，不被桌面或 Host 产品构建引用。服务器实际结果以 `.tmp/e2e/spikes.json` 和 `docs/archive/progress/02-fix-and-e2e.md` 为准，不能把脚本存在视为验收通过。

## 运行

先完成 `scripts/e2e/e2e_prepare.py` 提供的专用测试用户、密钥及主机指纹准备，再在本机执行：

```bash
python spikes/e2e_spikes.py
```

复用 `scripts/e2e/e2e_common.py` 的 Paramiko 连接、端点无关主机指纹固定与结果保存逻辑。本机需要 Paramiko。仅从环境变量读取 `AGENTDESK_E2E_HOST`、`AGENTDESK_E2E_PORT`、`AGENTDESK_E2E_ROOT_USER`、`AGENTDESK_E2E_ROOT_PASSWORD`；缺少任一变量打印变量名并成功跳过。脚本始终以 `agentdesk-test` 连接，不使用 root，不登录 CLI，不发送模型请求。`--skip-cli-install` 可禁止新装 Codex，只检查已有 CLI 与 Python SDK 随附的 Claude。

脚本当前只支持已探测的 Linux x86_64 服务器。安装和请求分别限时，公网下载带超时和 SHA256 校验。失败只输出固定阶段、退出码、ABI/网络失败布尔；CLI 的原始认证结果、账号字段、错误原文均不写报告。模型列表只输出数量与字段名，不能把列表非空当作该账号具备模型权限的证明。

## S6：独立运行时与进程存活

每次生成唯一 runId，新建以下目录：

- `~/.agentdesk/spikes/<runId>/`：固定版本的 musl 静态 uv、uv 管理的 Python 3.12、独立虚拟环境、可选的官方 Codex musl 二进制。
- `/home/agentdesk-test/agentdesk-e2e/spikes-<runId>/`：脚本、缓存、阶段输出、PID、Unix socket。所有删除及进程清理都局限在此目录，既有安装文件不覆盖。

uv 的官方压缩发行包由本机校验后通过 SFTP 独占创建上传，启用 pipeline，并设置通道及总上传时限；只在专用 E2E 目录内校验、解包，再通过 noclobber 新建安装目录中的静态二进制。这样避免在高 RTT 连接中上传体积更大的裸二进制。远端使用 `uv python install 3.12` 和 `uv venv --managed-python --python 3.12`。环境变量固定 Python、缓存、临时目录，不修改 shell 配置、不依赖系统 Python、不执行 sudo。`uv pip install --only-binary :all:` 安装官方 `claude-agent-sdk`，运行前从官方 PyPI 元数据取得并记录确切版本。只接受二进制 wheel，避免隐式编译或安装系统依赖。

`socket_service.py` 以 `setsid nohup` 启动，关闭全部 SSH 标准流，Unix socket 的 mode 为 0600。关闭整个 SSH 连接后重新连接，通过 `direct-streamlocal@openssh.com` 读取 echo，确认进程仍存活。实验服务最长 180 秒，正常路径及异常路径均清理已验证归属的进程、socket 和 PID。

只读检查 `systemctl --user show-environment` 和 `loginctl show-user --property=Linger`。若用户管理器可用，使用 `systemd-run --user` 创建临时用户单元，检查断开 SSH 后的存活并停止；不写持久化 unit，不开启 linger。若不可用，明确跳过该分支并保留原因，不能把 `setsid/nohup` 成功推断为 systemd 成功。验证期间不修改 sshd、防火墙或系统服务。

Paramiko 没有公开的 streamlocal 便捷接口，本实验按 OpenSSH 报文格式使用 Paramiko 的私有通道注册接口；它仅用于实验传输。产品仍使用 russh，russh 的完整 S1 验证由 Rust E2E 负责，不能用此实验替代。

## S3 / S4：不需要登录的 Agent 检查

`agent_probe.py` 在独立 Python 中执行以下检查，所有原始 stdout/stderr 仅在内存中解析后丢弃：

1. 导入 Claude Agent SDK，记录版本、必要导出、客户端方法及 Options 字段是否存在。按官方 SDK 源码定位 `claude_agent_sdk/_bundled/claude`；已有 PATH 中的 Claude 优先用于登录状态检查，并记录是否使用 bundled CLI。
2. 执行 `claude --version`、`claude auth status`，只输出版本、可运行性和已登录布尔。CLI 无法启动时标记 ABI 或可执行文件问题，不将其误判为单纯未登录。
3. Codex 不存在时在普通用户的专用 E2E 目录中限时下载官方 musl 发行包与同版本 bwrap 资源，校验固定 SHA256 后解包，分别通过 noclobber 新建安装目录中的 `codex` 和 `codex-resources/bwrap`。不需要 Node、npm 或系统 bwrap；已有 CLI 使用原版本，不覆盖。执行 `codex --version`、`codex login status`，只输出状态。
4. 启动 `codex app-server --listen stdio://`，等待 `initialize` 成功，再发送 `initialized`，请求 `account/read`（`refreshToken: false`）和 `model/list`。记录返回字段、账号是否存在、需要认证布尔、模型条数、首个模型的字段名和分页布尔。绝不输出账号内容，也不发起刷新、登录、审批放行或 turn。
5. 所有 RPC 都有时限和单帧上限；结束时终止 app-server 进程组并确认退出，超时升级为 SIGKILL。未完成模型调用不等于流式、工具、审批、中断、恢复或沙箱档位已通过。
6. 附加执行 `codex sandbox linux --help` 及仅输出固定字符串的本地命令，测试沙箱是否能初始化，不经过模型。该结果不能替代 `thread/start`、ssh 到 Unix socket 或完整权限档位验证。

`environment_probe.py` 只读记录精确 Python 版本、linger 的系统默认 Polkit 授权要求、用户 bus 是否存在、所有 Spike runId 目录，以及实验服务进程/PID/socket 是否残留。它不会执行 `enable-linger`，也不会输出任何进程命令行或账号数据。

未登录时，需要用户在测试服务器上以 `agentdesk-test` 身份自行执行 `claude auth login` 和 `codex login --device-auth`。安装目录未写入 PATH，可先将本次 `~/.agentdesk/spikes/<runId>/` 加入当前会话 PATH；Claude 若只有 SDK bundled CLI，使用脚本报告的 SDK 版本，在该虚拟环境中定位 `_bundled/claude` 后运行 `auth login`。若该二进制因旧 glibc 无法启动，应先选用受支持的服务器系统，不能通过安装旧版 Node 绕过后即宣称支持。脚本不会要求、读取或转存用户输入的登录凭据。

用户登录后仍需单独执行有授权的模型验收：新建、流式文本、工具调用、审批、中断、原生恢复，以及设计第 6.4 节两档 Codex 沙箱中 ssh 访问 Unix socket 的行为。本脚本始终不发送 prompt，这些分支必须在报告中列为跳过或后续验证。

## 与设计映射的差异及来源

以下是官方文档/源码核验结论，服务器实测必须另列：

- Claude Python 的原生元数据方法叫 `get_session_info()`，设计第 6.2 节的 `getSessionInfo` 是概念占位，不能直接用作 Python 方法名。
- Codex 当前的运行中追加输入接口是 `turn/steer`；设计表中“运行中再次 turn/start”不能直接作为适配契约，需按安装版本探测。
- SDK 自带 CLI，不应把“全局 claude 在 PATH 中”作为 SDK 已就绪的唯一判断；反过来，SDK 可导入也不能证明 bundled CLI 能在旧 glibc 上运行。
- uv musl 资产完全静态，不要求服务器安装 musl；Python standalone 与 Claude/Codex 二进制仍各自具有平台约束。不能从 uv 成功推出完整 Agent 栈支持 CentOS 7。
- 当前 Codex 单文件 CLI 资产缺少运行 Linux 沙箱所需的 `codex-resources/bwrap`。完整部署还需同版本官方 bwrap 资源。实测单文件情况下沙箱入口会在官方源码 `linux-sandbox/src/launcher.rs:60` 的资源缺失分支 panic；补齐资源后必须另测内核命名空间能力，不能将资源缺失误判为内核不支持。

本次核验的固定资产为 uv `0.13.5`、Codex `rust-v0.164.0`；摘要固定于 `e2e_spikes.py`，来自对应官方 GitHub Release API 的 `digest`。SDK 版本在每次执行前从 PyPI 解析并记录。

官方资料：

- [uv 平台支持](https://docs.astral.sh/uv/reference/policies/platforms/)、[Python 版本管理](https://docs.astral.sh/uv/concepts/python-versions/)、[环境变量](https://docs.astral.sh/uv/reference/environment/)。
- [uv 0.13.5 Release](https://github.com/astral-sh/uv/releases/tag/0.13.5)、[Codex rust-v0.164.0 Release](https://github.com/openai/codex/releases/tag/rust-v0.164.0)。
- [Codex App Server](https://developers.openai.com/codex/app-server)、[Codex CLI 命令参考](https://developers.openai.com/codex/cli/reference/)。
- [Claude Agent SDK Python 参考](https://code.claude.com/docs/en/agent-sdk/python)、[SDK CLI 定位源码](https://github.com/anthropics/claude-agent-sdk-python/blob/main/src/claude_agent_sdk/_internal/transport/subprocess_cli.py)、[Claude 安装要求](https://code.claude.com/docs/en/setup)、[Claude CLI 命令参考](https://code.claude.com/docs/en/cli-reference)。

## 回滚与残留

服务停止后保留本次安装、缓存和阶段输出，供复核；结果列出确切目录。脚本不删除 `~/.agentdesk/` 下的安装目录，因为本任务要求删除只在专用 E2E 目录进行。回滚时由用户另行确认删除对应 runId 目录，不能整目录清理其他版本。实验不创建账号、不改密码、不开启 linger，也不增加系统依赖。此脚本的清单应与用户创建阶段及其他 E2E 的变更清单合并。

## 2026-10-26 实测记录

测试服务器为 CentOS 7、x86_64、glibc 2.17。以下结果来自普通测试用户的实际执行，无 root、CLI 登录或模型调用：

| 项目 | 实测结果 |
| --- | --- |
| uv | `0.13.5` musl 静态发行包上传、SHA256 校验、安装和执行通过。 |
| Python | `3.12.15 (main, Oct 21 2026, 17:47:06) [Clang 23.1.3 ]`；uv 独立安装和 venv 创建通过，无需旧 build 回退。 |
| Claude | SDK `0.2.174` 可导入，bundled CLI `2.1.310` 可运行；`auth status` 返回未登录，无 GLIBC 启动错误。 |
| Codex | `0.164.0` musl 可运行；未登录。`initialize` → `initialized` → `account/read(refreshToken:false)` → `model/list` 全部成功。 |
| S6 socket | `setsid nohup` 服务在整个 SSH 连接断开后仍存活，socket 为 0600，重新连接后经 direct-streamlocal echo 成功；服务、PID、socket 均已清理。 |
| systemd 用户服务 | 未通过运行验证：用户 bus 不存在，用户管理器不可用；`linger=false`，未修改。 |
| linger 默认授权 | 只读系统 Polkit policy 中 `org.freedesktop.login1.set-user-linger` 的 `allow_any`、`allow_inactive`、`allow_active` 均为 `auth_admin_keep`，默认需要管理员授权。未执行开启命令。 |
| Codex Linux 沙箱 | 单文件 CLI 缺 bwrap 时入口 panic/exit 101；补齐官方 bwrap 后无 panic，但 help 与仅 printf 的本地进程仍 exit 1，命中用户命名空间初始化错误。该项失败，未继续 Unix socket 权限档位预检。 |
| 模型相关验收 | 流式、工具、审批、中断、恢复、完整 thread/start→ssh→Unix socket 均因未登录跳过；SDK 符号存在不构成这些行为已通过。 |
| 最终清理检查 | `activeSpikeWorkers=0`、`remainingServiceFiles=[]`，无实验服务进程、PID 或 socket 残留。 |

Codex 的脱敏返回形态：`initialize.result` 包含 `codexHome/platformFamily/platformOs/userAgent`；`account/read.result` 包含 `account/requiresOpenaiAuth/workspaceRouting`，账号为空且需要认证；`model/list.result` 为 `data/nextCursor`，目录 8 条、无下一页。完整模型字段名在本机忽略目录的结果 JSON 中，不保存模型内容或账号字段值。

补齐的 bwrap 取自相同 `rust-v0.164.0` Release，文件 `bwrap-x86_64-unknown-linux-musl.tar.gz`，归档 SHA256 为 `60cd1c139823eabac2e4544cc75081d02af5100cfd276efa378dd48a123e109d`。资源缺失分支依据[官方固定版本源码](https://github.com/openai/codex/blob/rust-v0.164.0/codex-rs/linux-sandbox/src/launcher.rs#L60)核对；补齐资源后的失败不能归类为 GLIBC 失败，也不能据此自动提升为 `danger-full-access`。

共留下三个 runId 的安装目录与 E2E 工作目录，均位于专用测试用户家目录：

- `840905def4b1`：首次裸二进制上传过慢，主动停止；未执行 uv，保留部分上传文件。
- `15ed3c2e0f58`：pipeline 裸上传调整期间主动停止；未执行 uv，保留部分上传文件。
- `9397564fc3e3`：最终有效执行，包含 uv、Python、venv、Codex 和新建的 `codex-resources/bwrap`。

每个 runId 对应 `~/.agentdesk/spikes/<runId>/` 与 `/home/agentdesk-test/agentdesk-e2e/spikes-<runId>/`。CLI 启动期间还产生 `.claude/`、`.claude.json`、`.claude.json.lock`、`.codex/`、`.pki/`；没有执行登录，不读取或导出这些目录中的账号内容。目录清单及最终状态在 `.tmp/e2e/spikes.json` 的 `audit` 中。

M2 应按可探测能力选择 Host 启动方式：本次服务器采用已验证的 `setsid nohup`，不能宣称 systemd 分支可用。部署 Codex 时固定同版本 CLI 与 bwrap 资源并验证摘要；Linux 沙箱初始化失败时明确提示环境不满足要求，保留完整 Agent/SSH 权限验收为后续步骤。
