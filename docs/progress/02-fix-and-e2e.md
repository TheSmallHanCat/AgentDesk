# 任务 02：审查修复、真实服务器验收与 M2 Spike

日期：2026-10-26。执行平台：Windows x64。服务器统一称“测试服务器”，不记录端点、密码、密码短语或账号认证内容。

结论：**审查问题已修复，本机要求的检查全部通过，M1 的五个真实 E2E 分组已实际通过。S1 的正反向数据流与 S6 的无 systemd 部署链路通过；公钥认证、systemd 用户服务、模型调用和 Codex 沙箱仍有明确的环境条件或验收缺口。不能宣布 S1/S3/S4/S6 全矩阵通过。**

产品改动保持 Vue/Rust/Python/协议边界；`spikes/` 不被产品构建引用，没有实现 Host 业务、图形 Agent 会话或 M3 执行通道。未修改上级设计文档、参考工程、sshd、防火墙或系统服务。

## 1. 审查问题与回归

| 问题 | 修复 | 验证证据 |
| --- | --- | --- |
| 已存密码被使用后清除 | `saveSecret` 改为 `null/缺省` 保持、`true` 保存、`false` 明确取消；认证失败也不静默清除。新增 `conn_secret_status`，只返回保存状态，弹窗默认勾选已保存凭据。 | 旧实现的 `review_saved_password_survives_unchanged_reconnect` 先失败；修复后覆盖三态、失败保留。浏览器检查默认勾选、未改动传 `null` 和切弹窗隔离。 |
| 目录下载超时、取消后 tar 残留 | `tar -czf -` 直接写 stdout，无远端临时包/大小头；60 秒为数据空闲期限，成功必须收到退出码 0。唯一 marker、GO 门闩、独立进程组；失败/取消关闭通道后显式 TERM/KILL 并检查存活；断开连接等待传输清理。 | `archive_idle_deadline_resets_for_each_data_chunk`、控制报文不能续期、非零/缺失退出码、预取消等协议回归；真实 210 MiB 下载、取消及 `.part` 清理通过。 |
| 旧服务器 setsid 不支持 `-w` | 实测退出 1 后改为复合块内后台 `setsid` + 外层 `wait`；显式保留 stdin 并关闭 job control，避免 GO 被外层 shell 消费或被 `/dev/null` 替代。 | 旧行为 shim 先复现失败；实际 Git Bash 进程回归验证可解包 tar 流、准确传播 tar 退出码 2、授权前关闭无输出；服务器重跑通过。 |
| ecdsa/rsa-only 被误判变化 | 根据已知算法调整 `preferred.key`；只比较同算法公钥，另一算法要求重新确认。RSA 签名哈希变体按同一种密钥算法处理。 | 本机双主机算法服务的 `review_ecdsa_only_record_selects_matching_host_algorithm`；真实 ecdsa-only 记录通过，变化指纹拒绝。 |
| 根目录保护不足 | 拒绝任一同 host/port 已登记项目根及祖先（含其他连接、其他用户和归档项目）、远端 HOME 及祖先、一级系统目录；检查规范化路径及实际父目录，删除不跟随末级软链接。 | 单元测试覆盖跨连接/用户/归档、不同 host/port 隔离；真实 SFTP 拒绝规则和专用目录创建/重命名/删除通过。 |
| 本机另存/诊断可指定任意路径 | `editor_save_local({content})`、`diag_export()` 由 Rust 打开原生保存对话框；命令不接受前端路径，取消返回 `null`，保留重名编号。 | `native-save-contract.test.ts` 锁定 IPC 边界；真实 WebView 启动/IPC 回归通过。保存对话框的人工选取/取消仍列在第 6 节。 |
| 终端 ack 丢失耗尽信用 | 后端累积释放 `seq ≤ N`；前端合并确认并重试失败，销毁停止重试。`term_reset` 在新前端初始化时关闭旧 PTY、清信用，组件销毁关闭其资源。 | 旧实现先复现 ack(8) 仅释放 1/8；累计释放、目标拒绝、重载清理及前端三个重试/合并/销毁回归通过。 |
| 保存替换破坏硬链接/元数据 | 可原子替换时临时文件 + rename；硬链接、父目录不可写、属主/组不宜复现、扩展/default ACL 或不能核验 ACL 时原 inode 写入，完整预读和读回比对后提示非原子风险。特殊权限位文件明确拒绝，force 不绕过。 | 实际执行保存策略的七组行为回归；真实硬链接共享内容、inode/mode/uid/gid、父目录不可写、默认 ACL 不扩大访问、特殊 mode 拒绝通过。 |
| 非 POSIX 登录 shell 解释脚本失败 | 非交互入口统一 `exec /bin/sh -s` + stdin。PTY 引导后仍进入用户 shell，csh/tcsh 的 CLI 启动分支不用不兼容的 `-l -c` 组合。初始关闭 ECHO，引导完成恢复，避免脚本显示在终端。 | 引导引用/分支单测、本机 PTY、真实 bash/中文/引号目录/PATH/resize/退出码和不回显引导脚本通过；fish/tcsh 未安装，真实分支待验。 |
| known_hosts 并发重复写 | check + learn 在同一把锁内，已可信的重复确认幂等返回，不追加重复行。 | 16 线程并发确认只写一行。 |
| workspace 串号校验无效 | 响应与当前 RequestGuard 的全部 Target 字段及序号比较；项目变更立即失效。已有资源仍保留创建时归属。 | 两个新回归在旧 workspace 上均失败，修复后覆盖切走再切回、同项目后请求先返回；浏览器迟到文件/连接隔离通过。 |
| CRLF/Tab 待查 | 确认 russh 0.65 按空格分割导致 Tab 条目不识别；读取时规范化普通记录，再复用其哈希主机匹配，不回写系统文件。 | 含 CRLF/Tab 的旧实现回归先失败，修复后本机及真实 ecdsa-only 文件通过。 |

`App.vue`、`FileTree.vue` 及前端源码已按 Prettier 统一展开；远端归档脚本拆为多行 `.sh` 资源。新增 `.prettierrc.json`、忽略生成文件的配置、`pnpm format:check` 和 CI 步骤。格式化后发现的 Vue 多语句事件表达式改为具名函数，生产构建和工作台回归通过。

保存取舍：原地写入保留硬链接、属主、组和 ACL，但中途断线可能留下不完整内容，不能宣称原子保存；前端内容不因失败而丢失。原子路径保持普通 mode/属主/组，不承诺复制所有自定义扩展属性。mtime/size 仍为乐观检测，不能检测相同大小和 mtime 的全部并发改写，也不构成文件锁。

## 2. 服务器只读探测

| 项目 | 实测 |
| --- | --- |
| 系统 / 架构 | CentOS 7 / x86_64 |
| 内核 / glibc | `3.10.0-957.1.3.el7.x86_64` / glibc 2.17，低于设计的 glibc 2.28 支持基线 |
| 初始 PATH | 有系统 Python、getfacl、tar、gzip、setsid、nohup、systemctl、loginctl、ss；未发现 Python 3、Node、npm、Claude、Codex、fish、tcsh |
| SSH 有效策略 | `PubkeyAuthentication=no`、`PasswordAuthentication=yes`；没有为验收修改配置 |
| streamlocal | `AllowStreamLocalForwarding=yes`、`StreamLocalBindMask=0177`、`StreamLocalBindUnlink=no` |
| 用户服务 | `systemd --user` 不可用；用户运行目录存在、user bus socket 不存在；linger=false |
| linger 权限 | 只读 policy 的 `allow_any/allow_inactive/allow_active` 均为 `auth_admin_keep`，默认需要管理员授权；未执行 enable-linger |

先探测，后创建专用用户和密钥。测试配置只在内存 SQLite，应用 known_hosts 在系统临时目录；root 密码仅从环境读取，普通测试用户随机密码和密钥密码短语按任务授权放在被忽略的 `.tmp/e2e/`。结果文件不含端点或秘密，仓库扫描通过。

## 3. E2E 矩阵及本机检查

入口与复现步骤见 `scripts/E2E.md`。五个 `real_server.rs` 分组分别通过 `scripts/e2e_run.py --filter ...` 实际执行，最终均退出 0，共输出 20 条 PASS 和 5 条条件 SKIP。默认 `cargo test` 忽略这五组；缺环境变量的脚本和测试也明确跳过，CI 不依赖服务器。

| 验收项 | 结果 | 说明 |
| --- | --- | --- |
| root / 普通用户密码、错误密码 | 通过 | 正确认证成功、错误密码被拒；后续绝大多数操作使用普通用户。 |
| 无密码短语/加密 ed25519、RSA PEM、错误密钥 | 跳过：服务器条件 | 已生成三把授权测试密钥和一把不授权密钥；实测正确公钥也被服务端拒绝，只读配置明确禁用公钥认证。不能把此拒绝当成密钥功能通过。本机密码/加密私钥协议回归通过，不能替代该真实矩阵。 |
| Windows OpenSSH agent / Pageant | 跳过 / 需人工 | 本机 agent 服务未运行；未启动服务、未添加密钥。未发现 Pageant。服务器还禁用公钥认证。 |
| 首次指纹、变化拒绝、ecdsa-only | 通过 | 临时应用 known_hosts 模拟变化；真实 ecdsa-only CRLF/Tab 协商成功。 |
| Shell/PTY | 通过 | 空格、单引号和中文目录；`.bashrc` PATH、中文输出、123×37 resize、退出码 23；引导脚本不显示。 |
| SFTP 与文件保护 | 通过 | 列表/隐藏文件/软链接/悬空链接/权限拒绝；专用目录新建/重命名/删除；保护规则只测试拒绝，不对保护对象执行删除。 |
| 编辑 | 通过 | LF、CRLF、混合换行读写；2 MiB 接受、超限/非 UTF-8 拒绝；只读文件拒绝、外部追加冲突；硬链接与默认 ACL 回归。混合换行经 CodeMirror 编辑可能统一为 LF 的已有语义未扩大承诺。 |
| 下载 | 通过 | 单文件、重名 ` (1)`、约 210 MiB 不可压缩目录流式 tar.gz，归档含中文及 `CON`；本机 tar 可检查归档。取消后无 tar 进程、远端临时归档或本机 `.part`。下载分组约 54 秒；跨空闲期限的持续流由短时协议回归额外覆盖。 |
| 黑洞、退避、停止、不重放 | 通过 | 本机专用代理模拟，无服务器网络规则改动；多次独立实测均约 45 秒判离线。失败重连后恢复，重连中停止，旧 PTY 不重建；追加型文件副作用检查重复重放。 |
| 双连接隔离 | 通过 | 同时连接 root 与普通用户，交叉并发读操作多轮，返回用户身份始终正确；前端另有迟到目标自动化。 |
| S1 正向/反向流及清理 | 数据流通过，自动 unlink 有差异 | russh 正向连接 0600 socket；服务器 socket → russh 回调 → 本机随机回环 TCP echo 成功。取消后确认监听关闭，旧 OpenSSH 保留 inode，测试显式 unlink；断开后同样清理本次 socket。 |

本机最终检查：

| 命令 / 检查 | 结果 |
| --- | --- |
| `pnpm test` | 15 通过，0 失败 |
| `pnpm typecheck` / `pnpm build` / `pnpm format:check` | 全部退出 0；CodeMirror 原有 >500 kB chunk 提示仍存在 |
| `cargo fmt --manifest-path src-tauri/Cargo.toml --check` | 退出 0 |
| `cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` | 退出 0 |
| `cargo test --locked --manifest-path src-tauri/Cargo.toml` | 41 单元 + 11 本机 SSH 集成通过；5 个真实环境组默认 ignored，已另行实际运行 |
| `cargo build --locked --manifest-path src-tauri/Cargo.toml` | 退出 0 |
| `pnpm protocol:check` | 三份生成输出一致 |
| `uv run --project host python -m pytest host/tests -q` | 6 通过 |
| 工作台 Playwright / Windows 原生 smoke | 均通过；截图只使用回环 IPC fixture，无测试服务器画面或秘密 |
| 新 Python 脚本编译、缺变量跳过、`git diff --check`、秘密值扫描 | 通过 |

## 4. S1 / S6 / S3 / S4 结论

### S1

russh 0.65.0 的 `channel_open_direct_streamlocal`、`streamlocal_forward`、`cancel_streamlocal_forward` 和服务器主动回调，在真实 OpenSSH 上完成字节流与本机 TCP 对接，无需回退其他 SSH 实现。0600 权限通过。不能假定取消/断线自动删除 socket 文件，产品后续必须按创建归属显式清理。公钥/agent 矩阵仍受第 3 节条件限制，因此 S1 只能判为部分完成。

### S6

成功安装位于 `~/.agentdesk/spikes/9397564fc3e3/`：uv **0.13.5** 静态 musl、独立 CPython **3.12.15** 和 venv。全程普通用户、不使用 sudo、不替换系统 Python。版本原文为 `3.12.15 (main, Oct 21 2026, 17:47:06) [Clang 23.1.3 ]`。

`setsid nohup` 的独立 echo 实验进程关闭 SSH 标准流后启动；断开整个 SSH，再连接后仍存活，0600 Unix socket 经 direct-streamlocal echo 成功，随后进程/PID/socket全部清理。它验证部署原语，不是已实现的 Host RPC 产品。`systemd --user` 分支因缺 user bus 跳过，未开启 linger，未创建持久服务。

首次裸 uv 上传及流水线裸上传分别中止；最终采用压缩官方发行包上传、在专用目录校验 SHA256/解包后独占安装。Codex 则从服务器下载官方包并校验。M2 不宜逐块确认上传数百 MiB 裸二进制。

### S3：Codex

官方 musl Codex **0.164.0** 可运行，`codex login status` 确认未登录。没有登录、刷新令牌或发送 prompt。stdio 实测顺序为 `initialize` → `initialized` → `account/read({refreshToken:false})` → `model/list`。

| 方法 | 实测返回形态 |
| --- | --- |
| `initialize` | `userAgent`、`platformFamily`、`platformOs`、`codexHome` |
| `account/read` | `account=null`、`requiresOpenaiAuth=true`、`workspaceRouting` |
| `model/list` | `data`、`nextCursor`；8 条目录记录，无后续页。条目包括 `id/model/displayName`、推理强度、`inputModalities`、`hidden/isDefault`、升级和 service tier 字段；不输出账号/模型响应内容。目录存在不证明调用权限。 |

**部署修正及未通过项：**只上传 Codex 主文件会缺少配套 `codex-resources/bwrap`，沙箱入口实测 panic/退出 101；对照同版本官方源码定位后补齐官方 musl bwrap，panic 消失，但最小无模型 printf 探测仍在用户命名空间初始化阶段退出 1。尚不能确定具体内核/宿主策略根因，未改系统配置。因最小沙箱进程无法启动，未继续 Unix socket/SSH 权限档位测试，未选择或自动切换 `danger-full-access`。

设计第 6.2 节的运行中追加输入应以当前官方接口 `turn/steer` 为候选，不能把再次 `turn/start` 当契约；本次未登录，不把文档核验当成该方法、流式/工具/审批/中断/恢复的实测通过。第 6.4 节权限档位仍待完整验收。

### S4：Claude

Claude Agent SDK **0.2.174** 安装/导入成功，随 SDK 提供的 Claude **2.1.310** 可运行，`claude auth status` 确认未登录。没有 Node/npm 也能使用此 bundled CLI 路径；不能只通过 PATH 是否有全局 `claude` 判断 SDK 可用性。

运行时反射确认 `ClaudeSDKClient` 的 `connect/query/receive_response/interrupt/disconnect/get_server_info` 存在，Options 含 `session_id/resume/can_use_tool/permission_mode/include_partial_messages/system_prompt`，导出含 `StreamEvent`、`list_sessions/get_session_messages/get_session_info`。Python 方法名是 **`get_session_info()`**，设计中的 `getSessionInfo` 不能直接作为 Python 调用名。

未登录，故新建真实运行、流式、工具、审批/AskUserQuestion、停止与原生恢复均跳过；方法存在不等于行为验证。本次未尝试任何账号凭据。

官方依据与供应商源码链接集中在 `spikes/README.md`；所有实测版本和脱敏字段保存在 `.tmp/e2e/spikes.json`。

## 5. 服务器改动与回滚清单

以下有意保留供复核，没有自动删除账号或安装：

1. 新建 `agentdesk-test`，家目录 `/home/agentdesk-test/`，随机密码；系统用户/组数据库的相应记录。`useradd` 自动生成 `/var/spool/mail/agentdesk-test`，属于账号创建附属文件，未手动写入邮件。
2. 家目录的标准 `.bash_logout/.bash_profile/.bashrc`，归属标记 `.agentdesk-e2e-owned`，0700 `.ssh/` 与 0600 `authorized_keys` 中三把专用公钥。另在 `.bashrc` 追加有标记的测试 PATH；交互 PTY 正常产生 `.bash_history`，内容只有生成的验收命令。
3. `/home/agentdesk-test/agentdesk-e2e/`：文件/权限/链接/编码/ACL/特殊 mode、约 210 MiB 目录、PATH 小命令、黑洞不重放标记、S1 echo 脚本及验收工作文件。权限 000 的目录、500 的父目录、默认 ACL 和特殊 mode 都只在此范围。
4. 两次部分上传及成功安装三个版本目录：`~/.agentdesk/spikes/840905def4b1/`、`15ed3c2e0f58/`、`9397564fc3e3/`；对应 `agentdesk-e2e/spikes-<同一 runId>/` 工作目录。前两次仅部分上传，未形成可用安装；第三次含 uv、Python、venv、SDK、Codex 和配套 bwrap。缓存、公开发行包、校验/阶段输出保留在对应工作目录。
5. 官方 CLI 自行创建的专用用户目录/文件：`.claude/`、`.claude.json`、`.claude.json.lock`、`.codex/`、`.pki/`。没有写入登录凭据，未修改其他用户配置。

**系统级依赖安装：无。linger：未开启。sshd/防火墙/系统服务配置：未修改。iptables/ss -K：未使用，恢复命令不适用。** 本机黑洞代理在用例结束时取消任务并关闭连接；测试用 Vite 进程也已关闭。

末尾执行 `scripts/e2e_audit.py`，并结合 Spike 自身审计，确认测试归属后台进程、socket/PID 服务文件、远端临时归档归零。本机取消测试也确认 `.part` 归零。普通用户家目录与 authorized_keys 权限分别为 0700、0600。

回滚时可按上述路径识别测试内容；账号、邮件 spool、`~/.agentdesk` 和 CLI 目录的删除超出本次专用目录删除边界，应另行确认后处理。本任务未为了清理而删除这些保留文件，也没有删除或修改其他用户的数据。

## 6. 需要用户与人工完成的事项

- 如需完成真实密钥/agent 验收，提供允许公钥认证的测试环境，或由用户自行决定服务器策略；本任务不会修改 sshd。Windows agent/Pageant 需具备可运行环境后重测。
- 用户以 `agentdesk-test` 登录服务器后，自行执行 `~/.agentdesk/spikes/9397564fc3e3/codex login --device-auth` 和 `~/.agentdesk/spikes/9397564fc3e3/venv/lib/python3.12/site-packages/claude_agent_sdk/_bundled/claude auth login`。安装未修改 PATH，需使用完整路径。先解决 Codex 沙箱启动问题再做模型验收；不要把凭据交给脚本或写入仓库。
- 登录后按设计映射逐项验收新建、流式、工具、审批、AskUserQuestion、停止、原生恢复；Codex thread/turn 字段和 6.4 两档 SSH/socket 权限尚未实测。不要用界面镜像重建原生上下文。
- systemd 用户服务与 linger 生命周期需要在具备 user bus 的环境验证；本次 fallback 成功不替代该分支。
- fish/tcsh 实机、WebView2 中文 IME 候选/多行粘贴、100%/150% 缩放、四分屏十分钟吞吐和内存、真实 CLI 终端、系统通知点击/禁用、托盘及安装包完整验收仍需人工/后续专门性能测试。本次没有给出性能达标结论。
- 原生保存对话框的人工选取/取消、不同目录权限等桌面交互需补验；自动化已锁定不能由 WebView 直接指定路径的 IPC 边界。

## 7. 对 M2 的实施建议

1. **部署预检分层。** 分别检查 uv、独立 Python、SDK 导入、CLI 运行、认证状态、沙箱、socket 和 Host 握手。CLI 版本/握手成功不能代表工具执行可用；该服务器的 Codex 沙箱失败应显示为环境能力问题，不自动放宽权限。
2. **优先验证过的普通用户 fallback。** systemd 用户管理器可用时单独验证服务及 linger 授权；不可用时使用重定向完整、脱离 SSH 的 setsid/nohup 路径。Host 需有归属明确的 PID/socket/版本和升级校验；本次 echo 实验不能代替生产生命周期实现。
3. **安装完整发行资源。** 固定版本和 SHA256，压缩上传或服务器端官方下载；Codex 主文件与 bwrap 等配套资源一起验证。保留独立 staging/版本目录，不改用户的 CLI 配置，不覆盖其安装。
4. **适配器按实际能力上报。** Python 用 `get_session_info`；Codex 追加输入核验 `turn/steer`。允许 account/model 返回新增字段，模型选择来自目录；将“未登录”和“目录可读”分开。审批、恢复、追加和权限档位必须在真实登录后建立契约测试。
5. **平台基线保持保守。** 本次证明指定版本的 uv/Python/CLI 在 CentOS 7 可启动，尚未证明 Agent 全链可用；保留设计 glibc ≥ 2.28 的产品支持基线，不据此扩大承诺。跨主机别名身份映射、旧 OpenSSH socket 清理和当前沙箱问题已记入 `docs/QUESTIONS.md`。

主要提交：`4ef1bdc` 审查修复与格式检查；`52347dd` ACL/跨连接保护及旧 setsid；`4c7f89f` PTY 引导与回归；`aa14f69` 真实验收入口；`00ef061` 部署与 Agent Spike；`ed4849d` 追加型不重放验证。本报告另作文档提交。
