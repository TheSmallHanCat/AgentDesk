# AgentDesk 需求与系统设计（Claude 版）

版本：0.1
日期：2026 年 10 月 26 日
状态：草案，待评审
产品名：**AgentDesk 为代号**，正式名称确定后统一替换（应用标识、数据目录 `~/.agentdesk`、仓库名、安装包名）

本文与 GPT 版的[需求文档](./远程开发工作台需求文档.md)和[前端布局与交互规范](./前端布局与交互规范.md)配合使用。GPT 版已经把产品边界、原则、状态和验收写得很完整，本文不重复这些内容，重点写 GPT 版没有落到实处的部分：**架构决策、协议、数据模型、接口清单、页面结构、参数、里程碑和风险**。两份文档冲突时，以第 1 节的决策记录为准。

---

## 0 与 GPT 版的分工

| 主题 | GPT 版 | 本文补充 |
| --- | --- | --- |
| 产品边界、原则、认证合规 | 完整 | 沿用，不重复 |
| 执行模式与通道架构 | 留作待定项 D3，并沿用 CC Desk 的 WSS 方案 | **改为两种模式都走 SSH**，取消公网服务（第 3 节） |
| 远端守护进程 | 只写了职责 | 部署、升级、生命周期、目录结构、Unix socket（第 4 节） |
| 桌面与远端的协议 | 未涉及 | JSON-RPC 方法表、事件序号、补取、幂等（第 5 节） |
| Claude / Codex 接入 | 写了验证方向 | 统一事件模型、两个 Agent 的字段映射、能力矩阵，以及 Codex 沙箱会拦截 SSH 的问题（第 6 节） |
| 本机执行通道 | 写了原则 | 完整时序和 Windows 侧组件（第 7 节） |
| 数据模型 | 只有概念表 | 两端的实体、字段和存储位置（第 8 节） |
| 功能需求 | 按模块描述 | 编号、P0/P1/P2 优先级、逐条验收（第 9 节） |
| 前端 | 布局规范 | 路由、组件树、Pinia store、可复用的 pymss 文件（第 10 节） |
| Tauri 后端 | 未涉及 | 模块划分、command 与事件清单（第 11 节） |
| 技术选型 | 写了“验证后选定” | 推荐的具体库，以及需要 Spike 验证的项（第 14 节） |
| 计划 | 阶段 A–F | 带通过标准的 Spike 和里程碑、风险登记（第 15、16 节） |

---

## 1 决策记录

### 1.1 沿用用户已确认的事项（来自 GPT 对话）

| 编号 | 决策 |
| --- | --- |
| C-01 | 两种工作方式都要做，放在同一个工作台里：SSH 远程开发（项目在服务器上），以及云端 Agent 操作本机项目（项目在本机） |
| C-02 | 最终交付桌面软件，前端布局和技术栈与 pymss-studio 一致，聊天交互参考 CC Desk Tunnel、ChatGPT、Open WebUI |
| C-03 | 首版接入 Claude Code 和 Codex，两者都要有完整的图形对话流程 |
| C-04 | 先发布 Windows，macOS 和 Linux 逐步补齐，平台相关代码从首版起就隔离 |
| C-05 | 首版面向个人，支持多服务器、多设备；团队能力以后扩展 |
| C-06 | 计划在 GitHub 公开发布，长期迭代 |
| C-07 | 服务器上运行**未修改的官方 CLI**，用户在服务器上走官方流程登录；桌面只负责转发提示词和交互、展示进度，不直接调用模型 API，也不重写 Agent 循环 |

### 1.2 本轮新确认的事项（2026-10-26）

| 编号 | 决策 | 影响 |
| --- | --- | --- |
| **D-01** | **两种模式都只依赖 SSH**：桌面经 SSH 在服务器上安装并启动 Agent Host；本机模式用 SSH 反向转发建立执行通道。服务器不需要公网端口、Docker 或 TLS 证书 | 不再沿用 CC Desk 的 WSS 服务。浏览器和手机访问留到以后作为 WSS 可选入口（协议层预留传输抽象） |
| **D-02** | **Agent Host 用 Python 编写**（与 pymss 的 Rust + Python Worker 分工一致），用 uv 管理独立的 Python 运行时 | 服务器不依赖系统自带的 Python；Claude 侧用官方 `claude-agent-sdk`（Python） |
| **D-03** | **首版 SSH 认证支持密码、私钥文件（含密码短语）、ssh-agent / Pageant，并能导入 `~/.ssh/config`** | GPT 版 FR01 中“私钥为扩展候选”改为 P0 |
| **D-04** | 产品名暂用代号 AgentDesk | 正式命名前不申请包名和域名 |
| **D-05** | **Codex 的沙箱权限由桌面程序在启动时下发**：桌面经 SSH 让 Host 拉起 Codex 时，按项目的执行模式带上权限参数（见 6.4 节） | 本机模式不依赖用户手动改服务器上的 Codex 配置；也不修改用户的 `~/.codex/config.toml` |
| **D-06** | **新项目开源，接受 AGPL-3.0**，可以直接复用 pymss-studio 的界面外壳代码 | 仓库许可证定为 AGPL-3.0；随包的第三方组件在 NOTICE 中列明 |
| **D-07** | **界面文案全部走 vue-i18n**，默认简体中文，首发附带英文 | 代码中不写死界面文案；沿用 pymss 的 i18n 键一致性测试 |
| **D-08** | **托盘常驻和系统通知进入 v0.1** | US-20、SYS-01、SYS-02 改为 P0 |
| **D-09** | **服务器默认可以访问外网**，不做离线安装包 | Host 和 CLI 都在线安装；ENV-06 取消 |
| **D-10** | **登录做成软件内的图形流程**：自动检测登录状态，桌面转交服务器 CLI 给出的授权链接或设备码，用户回填的代码再转交回 CLI | 4.4 节重写；ENV-04 验收标准提高；用户不需要阅读登录文档 |
| **A-03** | **本机安全边界是服务器上的这个 Linux 用户**。以该用户运行的任何程序在通道开启期间都能连接本机；按项目授权仅防误用，不对抗同用户恶意程序。Host 强制检查项目授权并支持单项目撤销，最后一个项目撤销或设备断开立即删除设备密钥；本机模式拒绝 plan | 首次授权提示、6.4 / 7 / 12 节与 README 必须如实说明 |

### 1.3 本文提出、待评审的决策

| 编号 | 建议 | 理由 |
| --- | --- | --- |
| P-01 | 桌面端的 SSH 用 Rust 的 `russh` 在进程内实现，不调用系统 `ssh.exe` | 需要在同一连接上做 PTY、SFTP、Unix socket 转发和反向转发，进程内实现方便统一管理生命周期和错误 |
| P-02 | Agent Host 只监听 Unix socket（`~/.agentdesk/run/host.sock`，权限 0600），桌面通过 SSH 的 `direct-streamlocal` 通道连接 | 不占用 TCP 端口，服务器上的其他用户无法连接 |
| P-03 | 编辑器用 CodeMirror 6 | 搜索、撤销和语法高亮几乎没有额外成本。GPT 版沿用 RemoteShell 的“无撤销、无搜索”，属于不必要的退步 |
| P-04 | 秘密统一存放在操作系统凭据库（`keyring` crate：Windows 凭据管理器、macOS 钥匙串、Linux Secret Service） | 跨平台方案一致，不用自己做 DPAPI 文件加密 |
| P-05 | 本机执行端首版沿用 CC Desk 已验证的做法：随包分发 Win32-OpenSSH 的 `sshd` 和 PowerShell 7；“Rust 内嵌 SSH 服务端”作为 Spike 备选 | 优先使用已经被验证过的方案 |

---

## 2 产品定位与用户故事

### 2.1 一句话定位

在一个桌面应用里管理多台服务器和多个项目。项目放在服务器还是本机都可以，用图形聊天驱动服务器上的官方 Claude Code 或 Codex 完成开发；需要时随手打开终端、编辑器和文件管理。

### 2.2 目标用户

| 画像 | 典型诉求 |
| --- | --- |
| 有云服务器的个人开发者 | 让 Agent 在服务器上长时间跑任务，桌面随时查看，断网后能接着看 |
| 本机工具链很重的开发者（Windows 客户端、游戏、嵌入式） | 项目和编译环境留在本机，Agent 跑在低配服务器上 |
| 有多台电脑的人 | 公司和家里两台电脑连同一台服务器，会话列表统一 |

### 2.3 用户故事（优先级：P0 首版必须，P1 首版尽量做，P2 后续）

| ID | 故事 | 优先级 |
| --- | --- | --- |
| US-01 | 作为用户，我能添加一台 SSH 服务器（密码或私钥），首次连接时确认主机指纹 | P0 |
| US-02 | 我能从 `~/.ssh/config` 一键导入已有主机 | P0 |
| US-03 | 我能在服务器上选一个目录作为项目，用 Claude 或 Codex 新建图形会话并对话 | P0 |
| US-04 | 我能看到 Agent 的流式回复、思考摘要、工具调用（参数和结果可以展开） | P0 |
| US-05 | Agent 请求权限或向我提问时，我能在会话里批准、拒绝或回答 | P0 |
| US-06 | 我能随时停止运行；网络断开再连上后，会话内容自动补全 | P0 |
| US-07 | 我能选一个本机目录作为项目，让服务器上的 Agent 操作本机文件、执行本机命令 | P0 |
| US-08 | 我能在当前项目目录下打开 Shell、Claude 或 Codex 终端，开多标签、最多四分屏 | P0 |
| US-09 | 我能浏览项目文件，在内置编辑器里修改保存，新建或删除目录，打包下载 | P0 |
| US-10 | 服务器缺少 Agent Host、Node 或 CLI 时，看到安装方案后由我确认安装 | P0 |
| US-11 | CLI 没有登录时，我能在桌面里完成官方登录流程 | P0 |
| US-12 | 项目可以收藏、搜索、归档，并设置默认 Agent 和模型 | P0 |
| US-13 | 运行中可以追加消息，由 Agent 自己决定何时采纳 | P1 |
| US-14 | 可以切换模型、推理强度、审批模式 | P1 |
| US-15 | 能看到上下文占用并主动压缩；可以分叉会话、编辑后重发 | P1 |
| US-16 | 另一台电脑打开同一服务器时，能看到并接手会话 | P1 |
| US-17 | 拖拽本机文件上传到远端目录 | P1 |
| US-18 | 查看额度和调用统计 | P2 |
| US-19 | 定时任务：到点向某个会话发送预设提示词 | P2 |
| US-20 | 托盘常驻、系统通知（运行完成、等待审批） | P0 |
| US-21 | 从浏览器或手机访问（WSS 入口） | P2 |
| US-22 | 只读查看 Git 状态；之后支持 worktree 并行会话 | P1 / P2 |

---

## 3 总体架构

### 3.1 拓扑

```text
┌──────────────────────── Windows 桌面（Tauri） ────────────────────────┐
│ Vue 前端  ──invoke / Channel──▶  Rust 核心                             │
│                                  ├─ SshManager（russh，每台服务器一条连接）│
│                                  ├─ HostClient（JSON-RPC over SSH 通道）│
│                                  ├─ PtyManager / SftpManager            │
│                                  ├─ LocalExecutor（本机模式）           │
│                                  │   └─ 回环 sshd + pwsh7（随包，按需启动）│
│                                  └─ Store（SQLite + keyring）           │
└──────────────┬────────────────────────────────────────────────────────┘
               │ 一条 SSH 连接（多路复用以下通道）
               │  ① session/exec：PTY 终端、安装命令
               │  ② sftp：文件浏览、编辑、上传下载
               │  ③ direct-streamlocal → host.sock：控制协议
               │  ④ streamlocal-forward（反向）：本机执行通道（仅本机模式）
               ▼
┌──────────────────────── Linux 服务器（普通用户） ─────────────────────┐
│ Agent Host（Python，常驻）                                             │
│  ├─ RPC Server（Unix socket，0600）                                    │
│  ├─ SessionManager + EventLog（SQLite）                                │
│  ├─ ClaudeAdapter ── claude-agent-sdk ── 官方 claude CLI ──┐           │
│  ├─ CodexAdapter ── codex app-server（stdio JSON-RPC）─────┤── 模型服务 │
│  └─ DeviceGateway：设备入口 ~/.agentdesk/devices/<id>/      │           │
│                     （本机模式下 Agent 用 ssh 连到这里）◀────┘           │
└──────────────────────────────────────────────────────────────────────┘
```

### 3.2 两种执行模式的数据流

| | 远端项目（SSH 远程开发） | 本机项目（云端 Agent 操作本机） |
| --- | --- | --- |
| 项目文件位置 | 服务器 | 桌面电脑 |
| Agent 进程 | 服务器上的 Agent Host 子进程 | 服务器上的 Agent Host 子进程（相同） |
| Agent 工作目录 | 项目目录本身 | `~/.agentdesk/native/<projectKey>/`（存放原生会话、记忆和设置） |
| Agent 读写文件、执行命令 | 原生工具直接操作服务器 | 原生 Bash 执行 `ssh -F <session.conf> device-script` → 反向通道 → 本机 sshd → pwsh |
| 桌面上的文件浏览和编辑 | SFTP | 本机文件系统（Rust 直接访问） |
| 桌面上的终端 | 远端 PTY | 本机 PTY（`portable-pty` / ConPTY）；Agent CLI 终端仍在远端 |
| 前提 | SSH 已连接、Host 就绪、CLI 已登录 | 以上全部，再加执行通道就绪 |

两种模式的控制面完全相同，区别只在“项目在哪”和“Agent 怎么碰到项目”。所以前端、协议和会话模型只需要实现一套。

### 3.3 为什么改为统一走 SSH（D-01）

| 维度 | CC Desk 的 WSS 服务 | 统一走 SSH |
| --- | --- | --- |
| 服务器部署 | Docker + 证书 + 服务凭据 + 开放端口 | 只要 SSH 能登录 |
| 攻击面 | 公网多一个端口，安全取决于服务凭据 | 不新增端口，复用 SSH 的认证 |
| 连接栈 | SSH（远端模式）和 WSS（本机模式）两套 | 一套 |
| 多设备 | 原生支持 | 每台设备各自建 SSH，Host 按设备 ID 区分，可行 |
| 浏览器 / 手机 | 原生支持 | 需要以后加 WSS 入口（P2） |
| 断线保活 | 服务端常驻 | Host 常驻（`systemd --user`，否则 `setsid nohup`），效果相同 |

代价：浏览器访问要以后单独做，而且需要 SSH 账号的人才能使用。对个人工具来说可以接受。

### 3.4 进程与生命周期

| 进程 | 由谁启动 | 什么时候结束 |
| --- | --- | --- |
| Agent Host | 桌面首次连接时检测，不存在则启动；优先注册 `systemd --user` 服务 | 空闲（没有运行、没有设备连接）满 `host.idle_exit`（默认 24 小时）后自己退出；用户可以在环境页停止 |
| Claude CLI（SDK 子进程） | Host，每个运行一个 | 一轮结束、没有后台任务时退出；下一轮用 `resume` 重新拉起 |
| codex app-server | Host，每台服务器常驻一个，多个 thread 复用 | Host 退出或崩溃重启时 |
| 远端 PTY | 桌面经 SSH 打开 | 关闭标签或 SSH 断开时（首版不保活） |
| 本机 sshd | 桌面，本机模式连接时启动 | 连接断开或桌面退出；由 Job Object 回收 |

---

## 4 Agent Host 设计

### 4.1 远端目录结构

```text
~/.agentdesk/
├── host/
│   ├── current -> versions/0.1.0        # 原子切换
│   ├── versions/0.1.0/                  # wheel、锁文件
│   └── uv/                              # 随包上传的 uv 静态二进制
├── runtime/python-3.12/                 # uv 管理的独立 Python
├── venv/0.1.0/                          # 每个版本一个虚拟环境
├── run/
│   ├── host.sock                        # 控制 socket，0600
│   ├── host.pid
│   └── host.lock                        # flock，防止多实例
├── state/
│   ├── host.sqlite                      # 会话索引、事件日志、设备、项目
│   └── host.log                         # 滚动日志，脱敏
├── native/<projectKey>/                 # 本机项目的 Agent 工作目录
├── devices/<deviceId>/                  # 设备入口（固定路径）
│   ├── ssh.conf                         # Host device / device-script
│   ├── tunnel.sock                      # 反向转发落点
│   └── keys/                            # 本次连接的临时私钥和 known_hosts，0700
└── session-ssh/<sessionId>.conf         # 只有一行 Include，指向设备入口
```

### 4.2 安装与升级

1. 桌面连接后执行探测脚本（只读）：`uname -sm`、glibc 版本、`$SHELL`、`~/.agentdesk/host/current` 的版本号、`systemctl --user` 是否可用、`node` / `npm` / `claude` / `codex` 的版本。
2. 根据结果生成**安装方案**，展示给用户：要上传什么、写到哪里、执行哪些命令。用户确认后才执行。
3. 通过 SFTP 上传 `uv`（musl 静态版）和 Host wheel，执行 `uv python install` 和 `uv venv` / `uv pip install`。**不依赖系统 Python，不使用 sudo。**
4. 服务器默认可以访问外网（D-09），Python 运行时和依赖在服务器上在线下载；网络失败时显示具体的下载地址和错误，不提供离线包。
5. 升级时安装到新的 `versions/<v>`，再切换 `current` 链接，旧版本保留一份用于回滚。有运行在进行时提示用户延后升级。
6. 桌面和 Host 握手时交换 `protocolVersion`：主版本不同则拒绝并引导升级，次版本不同则按能力降级。

### 4.3 官方 CLI 安装

CLI 的安装方式由适配器声明，安装命令在远端 PTY 里执行，用户能看到完整输出：

| 工具 | 首选方式 | 备选方式 |
| --- | --- | --- |
| Claude Code | 官方原生安装脚本（不需要 Node） | `npm i -g @anthropic-ai/claude-code` |
| Codex | `npm i -g @openai/codex` | 官方发布的独立二进制 |
| Node/npm（只在走 npm 时需要） | nvm 用户级安装 | 提示用户自己安装 |

安装命令通过登录 shell（`$SHELL -lc`）执行，这样能加载 `.bashrc` / `.zshrc` 里的 PATH（沿用 RemoteShell 的做法）。安装完成后重新检测；退出码为 0 但检测不到时，显示“安装失败”。

### 4.4 登录

**登录是软件内的图形流程，桌面只做“传话人”**（D-10）。用户不需要打开终端、SSH 到服务器，也不需要阅读任何登录文档。

流程：
1. 连上服务器、守护进程就绪后，自动检测每个 Agent 的登录状态，分为未安装、未登录、已登录（附账号类型）、检测失败四种。新建会话、环境页、聊天头部都会显示。
2. 用户在未登录的 Agent 旁点“登录”，守护进程在服务器上启动官方登录命令（放在后台 PTY 里，用户看不到终端）。
3. 守护进程从 CLI 输出中**解析**授权链接、设备码、“请粘贴授权码”之类的提示，转成结构化的 `login.prompt` 事件发给桌面。
4. 桌面弹出**登录对话框**：
   - 有授权链接时，显示“在浏览器中打开”按钮，同时给出复制链接的按钮；
   - 有设备码时，大字显示设备码，并提供复制按钮；
   - CLI 要求回填授权码时，显示输入框。用户在浏览器授权后把代码粘贴进来，桌面经 `login.respond` 交给守护进程，写入 CLI 的标准输入。
5. CLI 退出后，守护进程重新检测登录状态，对话框显示成功或失败原因。登录可以随时取消，取消时结束这个登录进程。
6. 解析失败（比如 CLI 改了输出格式）时，对话框自动退回“显示原始输出 + 输入框”的模式，保证还能登录。原始输出只在对话框里显示，不写日志。

各 Agent 的做法：
- **Claude**：运行官方 `claude auth login`，桌面转交授权链接，用户把浏览器页面上的代码回填。状态用 `claude auth status`。
- **Codex**：优先走设备码流程（`codex login --device-auth`，或 app-server 支持的设备码登录接口），用户在浏览器输入设备码即可，不需要回填。不能用回调到服务器本机地址的浏览器登录，因为用户的浏览器访问不到服务器的回环地址。状态用 `account/read`。
- 两者都支持“退出登录”，执行官方的 logout 命令。

安全边界：
- 授权链接、设备码、回填码只在“守护进程 ↔ 桌面对话框”之间传递一次，**不落库、不写日志、不进入事件表**。对话框关闭后，前端立即清除这些内容。
- 凭据由 CLI 自己保存在服务器上，桌面和守护进程都不读取、不复制。
- 桌面不提供账号密码表单，也不代替用户在浏览器里授权。

---

## 5 协议设计（桌面 ↔ Agent Host）

### 5.1 传输与帧格式

- 传输：SSH `direct-streamlocal` 通道连接 `host.sock`。协议层抽象成双向字节流，以后加 WSS 只需要换传输层。
- 帧格式：一行一个 JSON（NDJSON），遵循 JSON-RPC 2.0。消息分三类：请求 / 响应、通知（Host → 桌面的事件）、服务端请求（例如审批，Host 主动问桌面）。
- 单帧上限 4 MiB，超过的工具结果截断后附上 `truncated: true` 和完整长度，完整内容按需用 `event.blob` 拉取。

### 5.2 握手

```json
→ {"jsonrpc":"2.0","id":1,"method":"hello","params":{
     "protocolVersion":"1.0","client":"agentdesk-desktop/0.1.0",
     "deviceId":"dev_…","deviceName":"WORK-PC","os":"windows",
     "resume":{"lastSeq":10234}}}
← {"jsonrpc":"2.0","id":1,"result":{
     "protocolVersion":"1.0","host":"0.1.0","hostId":"host_…",
     "capabilities":{"claude":{…},"codex":{…}},"currentSeq":10260}}
```

`hostId` 在 Host 首次安装时生成并持久保存。桌面用它判断“两个连接配置其实是同一台服务器”（例如通过 IP 和域名分别添加了同一台机器）。

### 5.3 方法清单（首版）

| 域 | 方法 | 说明 |
| --- | --- | --- |
| 环境 | `env.probe` / `agent.status` / `agent.login.start` / `agent.logout` | 检测与登录 |
| 项目 | `project.list` / `project.upsert` / `project.archive` | Host 侧的项目索引（多设备共享） |
| 会话 | `session.list` / `session.create` / `session.rename` / `session.delete` / `session.fork` | `create` 指定 agent、projectId、执行模式 |
| 历史 | `session.history`（分页，按 seq 倒序）/ `event.blob` | 打开会话时按页拉取 |
| 运行 | `run.send` / `run.append` / `run.stop` / `run.stopTask` | `send` 必须带 `clientMsgId` |
| 控制 | `run.setModel` / `run.setPermissionMode` / `run.setEffort` / `run.compact` | 按能力矩阵开放 |
| 审批 | `approval.respond` | 回复 Host 发来的 `approval.request` |
| 订阅 | `sync.subscribe` / `sync.since` | 订阅会话事件；断线后从 lastSeq 补取 |
| 设备 | `device.register` / `device.tunnel.ready` / `device.list` | 本机模式的执行通道 |

### 5.4 事件模型

所有会话事件写入 Host 的 `events` 表，带**全局单调递增的 `seq`**，再推送给订阅方：

```json
{"jsonrpc":"2.0","method":"event","params":{
  "seq":10261,"sessionId":"ses_…","runId":"run_…","ts":"2026-10-26T08:00:00Z",
  "type":"item.delta","itemId":"itm_…","payload":{"kind":"text","delta":"好的，"}}}
```

| type | 含义 |
| --- | --- |
| `run.started` / `run.state` / `run.finished` | 运行状态：idle / running / awaiting_approval / awaiting_background / stopping / completed / failed / interrupted |
| `item.started` / `item.delta` / `item.completed` | 内容项：`text`、`reasoning`（思考摘要）、`tool_call`、`tool_result`、`file_change`、`plan`、`error` |
| `approval.request` / `approval.resolved` | 审批或提问；resolved 中带有决定方的 deviceId |
| `usage` | token、上下文占用（只在 Agent 实际返回时发送） |
| `session.updated` | 改名、模型、审批模式变化 |
| `user.message` | 用户消息落库（带 clientMsgId，多设备同步） |

**规则**
1. `item.delta` 只转发给在线订阅方，不落库；`item.completed` 落库一条完整内容（沿用 CC Desk 的做法，避免事件表膨胀）。
2. 断线重连：`sync.since(lastSeq)` 返回之后的已落库事件；正在进行的 item 再补一条当前快照。
3. 幂等：`run.send` 的 `clientMsgId` 在 Host 去重。桌面在**没收到确认**时不会自动重发，而是把消息标记为“状态未知”，由用户选择查看或重发。
4. 审批竞争：同一个 `approvalId` 只接受第一个回复，后到的回复返回 `already_resolved`，并附上已生效的决定。

### 5.5 错误码

`E_PROTOCOL_VERSION`、`E_NOT_LOGGED_IN`、`E_AGENT_NOT_INSTALLED`、`E_SESSION_BUSY`、`E_NATIVE_MISSING`（原生记录缺失）、`E_DEVICE_OFFLINE`、`E_TUNNEL_NOT_READY`、`E_CAPABILITY`（Agent 不支持）、`E_ALREADY_RESOLVED`。错误响应带 `stage` 和 `hint`，界面直接显示“哪一步失败、下一步怎么做”。

---

## 6 Agent 适配层

### 6.1 统一接口（Host 内部，Python）

```python
class AgentAdapter(Protocol):
    kind: Literal["claude", "codex"]
    async def capabilities(self) -> Capabilities: ...
    async def start_run(self, session: Session, input: UserInput, ctx: RunContext) -> RunHandle: ...
    async def append_input(self, run: RunHandle, input: UserInput) -> None: ...
    async def stop(self, run: RunHandle) -> None: ...
    async def respond_approval(self, run: RunHandle, approval_id: str, decision: Decision) -> None: ...
    async def native_exists(self, session: Session) -> bool: ...
```

适配器把原生事件翻译成 5.4 节的统一事件。**界面只认统一事件**，不针对某个 Agent 写特殊逻辑；两个 Agent 的差异由能力矩阵表达。

### 6.2 原生接口映射

| 统一概念 | Claude（`claude-agent-sdk` + 官方 CLI） | Codex（`codex app-server`，stdio JSON-RPC） |
| --- | --- | --- |
| 新建原生会话 | `ClaudeSDKClient(options)`，首轮固定 `session_id` | `thread/start` |
| 恢复 | `options.resume=<nativeId>`；先用 `getSessionInfo` 类接口确认存在 | `thread/resume` |
| 发送 | `client.query()` | `turn/start` |
| 追加输入 | 同一进程的流式 user input | 运行中再次 `turn/start` 的语义需要验证（可能要等当前 turn 结束），不支持时隐藏入口 |
| 流式文本 | `StreamEvent` 增量 / `AssistantMessage` | `item/agentMessage/delta` |
| 思考摘要 | `ThinkingBlock`（summarized） | `item/reasoning/*` |
| 工具调用 | `ToolUseBlock` / `ToolResultBlock` | `item/started` / `item/completed`（commandExecution、fileChange、mcpToolCall） |
| 审批 | `can_use_tool` 回调（含 `AskUserQuestion`） | 服务端请求 `item/commandExecution/requestApproval`、`item/fileChange/requestApproval` |
| 停止 | `client.interrupt()`，必要时结束进程 | `turn/interrupt` |
| 模型列表 | CLI 初始化返回的 supportedModels | `model/list` |
| 审批模式 | `permission_mode`：default / acceptEdits / plan / auto（bypass 需要用户在设置中开启并二次确认） | `approvalPolicy` + `sandbox` |
| 用量 | `ResultMessage.usage`、OTel 日志（P2） | `thread/tokenUsage/updated` 类通知 |

> 上表中的方法名以 Spike 时官方文档和实测结果为准。上表负责定义统一的语义，具体字段由适配器负责吸收差异。

### 6.3 能力矩阵（首版目标）

| 能力 | Claude | Codex | 界面行为 |
| --- | --- | --- | --- |
| 创建 / 恢复 / 流式 / 工具 / 审批 / 停止 | ✅ P0 | ✅ P0 | — |
| 追加输入 | ✅ P1 | ⚠️ 待验证 | 不支持时隐藏输入框的“追加”状态，发送按钮变为排队 |
| 切换模型 / 推理强度 | ✅ P1 | ✅ P1 | 来自原生列表，不写死型号 |
| 主动压缩 | ✅ P1 | ⚠️ 待验证 | — |
| 分叉 | ✅ P1 | ⚠️ 待验证 | — |
| 编辑重发 | ✅ P1（分叉到该消息之前） | ⚠️ 待验证 | — |
| 后台任务列表 | ✅ P1 | ❌ | — |

能力在握手时由 Host 根据**实际检测到的 CLI 版本**上报，不按 Agent 类型写死。

### 6.4 本机模式的关键问题

A-03 已确认：安全边界是服务器上的这个 Linux 用户；该用户的任何程序在通道开启期间都能连接本机。按项目授权防止误用，不提供对抗恶意程序的隔离。本机 Claude/Codex 禁止 plan；服务器沙箱只读无法保证 SSH 内的 Windows 操作只读。Codex 常规执行仅采用 A-02 第 1 档，不回退 danger-full-access。

1. **告诉 Agent 项目在本机。** 沿用 CC Desk 的提示词注入方式：Claude 用 `system_prompt.append`，Codex 用 `developerInstructions`（或 `baseInstructions`，待验证）。提示词只写 `session-ssh/<id>.conf` 这个固定路径和主机别名，不写临时密钥和端口，保证重连时系统提示词不变（提示缓存不会失效）。
2. **Codex 沙箱权限由桌面在启动时下发（D-05）。** Codex 默认的 `workspace-write` 沙箱会禁止网络访问，也可能读不到 `~/.agentdesk/devices/` 和 Unix socket，这样本机模式下的 ssh 会被拦截。处理方式：
   - 桌面根据项目的执行模式算出一份**权限配置**，经 SSH 控制通道交给 Host；Host 拉起 `codex app-server` 时用命令行 `-c` 覆盖项传入，或在 `thread/start` 的 `sandbox` / `approvalPolicy` 参数里传入（哪种生效以 S3 实测为准）。
   - 只用命令行参数和线程参数，**不改写**用户服务器上的 `~/.codex/config.toml`，不影响用户自己在终端里用 Codex。
   - 按最小权限依次尝试，S3 确定最终用哪一档：
     1. `workspace-write` + `sandbox_workspace_write.network_access=true` + `writable_roots` 加入 `~/.agentdesk/devices/<id>/`；
     2. 第 1 档仍然拦截 Unix socket 时，改用 `danger-full-access`。
   - 远端项目模式不下发额外权限，沿用 Codex 默认沙箱或用户在界面上选的审批模式。
   - 实际生效的沙箱模式写入会话状态栏。用了第 2 档时，界面提示“本机模式下 Codex 的沙箱限制不生效，安全边界由执行设备上的用户权限决定”。
   - 权限配置有版本号，随 Host 一起升级；Codex 改了参数名时，由适配器负责兼容。
3. **原生文件工具会读写 Linux。** Claude 的 Read/Edit 和 Codex 的 apply_patch 默认操作的都是 Linux 上的工作目录。提示词要引导 Agent 通过 `device-script` 操作本机文件；偶尔误操作 Linux 工作目录的风险是可以接受的（与 CC Desk 一致）。`fileChange` 类事件在本机模式下要标注“作用于服务器工作目录”，避免用户误以为改的是本机。
4. 换设备、换目录的处理沿用 CC Desk 的做法：在下一条用户消息前加一段说明，不改系统提示词。

---

## 7 本机执行通道（SSH 反向转发）

### 7.1 建立过程

```text
桌面                                   服务器（Agent Host）
 │ 已有 SSH 连接 + host.sock 控制通道        │
 │── device.register(deviceId, os) ────────▶│ 准备 devices/<id>/
 │◀─ {tunnelSocket, hostKeyPinRequired} ───│
 │ 生成本次连接的临时密钥：sshd 主机密钥 + 登录密钥（ed25519，只放在内存和临时目录）
 │ 启动回环 sshd（127.0.0.1:随机端口，只允许本次登录公钥，ForceCommand 强制运行随包 PowerShell stdin runner）
 │── streamlocal-forward(devices/<id>/tunnel.sock) ─▶│（SSH 协议层的反向转发）
 │── device.tunnel.ready(登录私钥, sshd 主机公钥) ─▶│ 写入 keys/、ssh.conf
 │                                          │ 探测：ssh -F ssh.conf device-script <<< 'echo ok'
 │◀─ forwarded-streamlocal 通道 ────────────│
 │ 把通道字节对接到 127.0.0.1:<sshd 端口>     │
 │◀─ tunnel.state = ready ─────────────────│ 探测通过后，界面才允许发起本机任务
```

`ssh.conf` 示例：

```text
Host device
  HostName localhost
  ProxyCommand nc -U ~/.agentdesk/devices/<id>/tunnel.sock   # 或用 Host 自带的 python 小工具，避免依赖 nc
  User agentdesk
  IdentityFile ~/.agentdesk/devices/<id>/keys/id
  UserKnownHostsFile ~/.agentdesk/devices/<id>/keys/known_hosts
  StrictHostKeyChecking yes
  ServerAliveInterval 10
  ServerAliveCountMax 2
Host device-script
  HostName localhost
  # 其余同上
  RemoteCommand pwsh -NoLogo -NoProfile -NonInteractive -File <stdin-runner.ps1>
```

### 7.2 要点

- 反向转发的落点是 **Unix socket（0600）**，不是 TCP 端口，服务器上的其他用户无法连接。
- 即使拿到了 socket，还要过内层 SSH 认证（临时密钥），通道本身不提供任何权限。
- 每次连接都更换密钥；设备入口的路径固定，所以 Agent 进程无感知。
- 设备离线时，新命令在入口处等待，最长 `device.wait_timeout`（默认 60 秒）；离线超过 `device.grace`（默认 5 分钟）后，属于该设备的运行标记为中断。
- **不是沙箱（A-03）。** 安全边界是服务器上的这个 Linux 用户；该用户下任何程序在通道开启期间都能连接本机，拥有桌面当前用户权限。每项目授权仅防止误用，不对抗同用户恶意程序；需要强隔离时须采用每项目独立 Linux 用户等系统措施。首次提示须如实告知。
- Host 维护本次控制连接已启用的 projectKey，run.send/run.append 强制检查；device.project.revoke 撤销单项目并删除其会话入口。至少有一个已授权项目且设备在线时才允许设备密钥存在；最后一个项目撤销或设备断开立即删除。
- `device-script` 读取标准输入作为 PowerShell 脚本执行，UTF-8、保留退出码（沿用 CC Desk 的经验：不能用 `-Command -`，它逐行处理，跨行的代码块会丢失）。
- Windows 侧执行命令时不弹出控制台窗口；sshd 和 pwsh 由 Job Object 归属，桌面退出时一并结束。

### 7.3 macOS / Linux 桌面（后续）

执行端换成系统 sshd 或随包 OpenSSH + `/bin/sh` 版本的 `device-script`；服务端逻辑不变。设备握手时上报 `os`，Host 据此选择提示词里那两句和平台有关的说明。

---

## 8 数据模型

### 8.1 桌面本机（`%APPDATA%\AgentDesk\`）

| 实体 | 关键字段 | 存储 |
| --- | --- | --- |
| Connection | id, name, host, port, user, authMethod(password/key/agent), keyPath, jumpHost?, hostId?, lastUsedAt, tags | `desktop.sqlite` |
| Secret | `agentdesk:<connectionId>:password` / `:passphrase` | 系统凭据库 |
| KnownHost | host:port, keyType, fingerprint, addedAt | 应用自己的 `known_hosts`；同时读取 `~/.ssh/known_hosts` |
| LocalProject | id, name, path（规范化后）, defaultAgent, defaultModel, pinned, archived, lastOpenedAt | `desktop.sqlite`（本机项目的“文件在哪”只有桌面知道） |
| RecentDir | connectionId, remoteUser, path, usedAt（每个连接 8 条） | `desktop.sqlite` |
| DeviceIdentity | deviceId（首次启动生成）, deviceName | `desktop.sqlite` |
| Draft | sessionId → 文本 | `desktop.sqlite` |
| Layout | 侧栏宽度、折叠状态、打开的标签（不含 PTY 状态） | `desktop.sqlite` |
| Settings | 主题、语言、快捷键、终端字体等 | `settings.json`（与 pymss 一致） |
| SessionCache | 最近打开会话的事件分页缓存（有上限） | `cache.sqlite`，可以随时清空 |

### 8.2 服务器（`~/.agentdesk/state/host.sqlite`）

```sql
projects(id, kind /*remote|local*/, device_id NULL, path, display_name,
         native_root, default_agent, created_at, archived_at)
sessions(id, project_id, agent /*claude|codex*/, native_id, native_root,
         title, model, permission_mode, effort, status, device_id NULL,
         created_at, updated_at, deleted_at)
runs(id, session_id, state, started_at, finished_at, stop_reason, origin_device_id)
events(seq INTEGER PRIMARY KEY, session_id, run_id, ts, type, item_id, payload_json)
approvals(id, run_id, kind, payload_json, state, decision_json, decided_by_device, decided_at)
devices(id, name, os, last_seen_at, online)
client_msgs(client_msg_id PRIMARY KEY, session_id, accepted_at)   -- 幂等表
```

- `projects` 中的远端项目由 Host 维护；本机项目只记录 `device_id + path`（用于提示词和工作目录分配），显示名称以桌面为准。
- 原生会话内容（`~/.claude/`、`~/.codex/`）永远不复制进 `events`；`events` 只是界面镜像。
- 保留策略：`events` 默认保留 180 天，可以配置；删除会话时先删除原生会话，成功后再删除镜像。

---

## 9 功能需求清单

格式：编号 / 需求 / 优先级 / 验收要点。GPT 版中已有的原则（不自动安装、破坏性操作要确认、迟到的响应不能作用到新目标等）全部继承，这里不再重复。

### 9.1 连接（CONN）

| ID | 需求 | 优先级 | 验收 |
| --- | --- | --- | --- |
| CONN-01 | 新增和编辑 SSH 连接：地址、端口、用户、认证方式 | P0 | 支持 `user@host:port` 和 IPv6 格式 |
| CONN-02 | 认证方式：密码 / 私钥（OpenSSH、PEM 格式，可带密码短语）/ ssh-agent（Windows 上为 OpenSSH agent 管道和 Pageant） | P0 | 三种方式都能连上测试机 |
| CONN-03 | 从 `~/.ssh/config` 导入：Host、HostName、Port、User、IdentityFile、ProxyJump | P0 | 导入后显示来源；不回写用户的配置文件 |
| CONN-04 | 主机指纹：首次连接确认，指纹变化时拒绝连接并显示新旧指纹对比 | P0 | 拒绝后不会建立任何通道 |
| CONN-05 | 跳板机（ProxyJump，单层） | P1 | — |
| CONN-06 | 密码默认不保存，勾选后存入凭据库；保存的密码失效时自动清除并重新提示输入 | P0 | 配置文件和日志中都没有明文 |
| CONN-07 | 心跳 15 秒，45 秒无响应判定断线；自动重连按指数退避（1、2、4…最长 30 秒），用户可以停止 | P0 | 认证被拒绝时不再重试 |
| CONN-08 | 同时连接多台服务器，各连接的状态、会话和请求相互隔离 | P0 | AC08 |
| CONN-09 | 多个连接配置指向同一个 hostId 时提示“同一台服务器” | P1 | — |

### 9.2 环境（ENV）

| ID | 需求 | 优先级 | 验收 |
| --- | --- | --- | --- |
| ENV-01 | 只读探测服务器的系统、架构、glibc、Host 版本、node、claude、codex，以及 systemd --user 是否可用 | P0 | 区分未安装、检测失败、版本未知 |
| ENV-02 | 生成 Agent Host 安装方案，确认后安装；安装过程有日志、可以取消 | P0 | 不使用 sudo，不改动 shell 配置文件 |
| ENV-03 | CLI 安装方案（4.3 节），在远端 PTY 中执行 | P0 | 安装后重新检测确认 |
| ENV-04 | CLI 登录状态自动检测；图形登录对话框（授权链接、设备码、回填码），桌面只做传话人（4.4 节） | P0 | 全程不需要打开终端；链接和授权码不进入日志或数据库；解析失败时能退回原始输出模式完成登录；可以取消、可以退出登录 |
| ENV-05 | Host 升级和回滚 | P1 | 升级失败时保留旧版本 |
| ENV-07 | 本机模式依赖检测：随包的 sshd、pwsh 是否完整，是否被安全软件拦截 | P0 | 被拦截时给出具体的文件路径 |

### 9.3 项目（PROJ）

| ID | 需求 | 优先级 | 验收 |
| --- | --- | --- | --- |
| PROJ-01 | 新建远端项目：路径输入框带实时补全（沿用 RemoteShell 的路径编辑体验）+ 最近目录 | P0 | 目录不存在时询问是否创建 |
| PROJ-02 | 新建本机项目：系统目录选择器 | P0 | — |
| PROJ-03 | 项目列表按“服务器 / 本机设备”分组；支持搜索、收藏、归档、恢复 | P0 | 归档不删除文件和原生会话 |
| PROJ-04 | 项目设置：显示名称、说明、默认 Agent、默认模型、默认审批模式 | P0 | 只影响新会话 |
| PROJ-05 | 只读 Git 状态（分支、是否有改动、领先 / 落后） | P1 | 非 Git 目录正常使用 |
| PROJ-06 | 同一项目有多个会话同时运行时，显示并发写入提示 | P1 | — |
| PROJ-07 | worktree 并行会话 | P2 | — |

### 9.4 会话与对话（CHAT）

| ID | 需求 | 优先级 | 验收 |
| --- | --- | --- | --- |
| CHAT-01 | 新建会话：选择 Agent（显示是否已安装、已登录）、模型、审批模式 | P0 | 前提不满足时给出具体原因和处理入口 |
| CHAT-02 | 消息流：Markdown、代码块（复制、语言标识）、表格、长内容折叠 | P0 | — |
| CHAT-03 | 思考摘要默认折叠；工具调用显示为一行摘要（图标、动作、目标、耗时、状态），点开看参数和结果 | P0 | 连续的工具调用合并为“工作步骤”组 |
| CHAT-04 | 文件修改类工具显示 diff 视图 | P1 | — |
| CHAT-05 | 审批卡片：显示命令或文件、风险说明（来自原生）、批准 / 拒绝 / 本会话总是批准（Agent 支持时） | P0 | 多设备同时操作时只有一次生效 |
| CHAT-06 | 提问卡片（Claude 的 AskUserQuestion）：单选、多选、自由输入 | P0 | — |
| CHAT-07 | 输入框：Enter 发送、Shift+Enter 换行、草稿按会话保存；运行中按钮变为停止 | P0 | 切换会话时草稿不串 |
| CHAT-08 | 输入框中 `@` 引用项目文件（只插入路径文本） | P1 | — |
| CHAT-09 | 粘贴图片作为附件 | P2 | 未实现前不显示上传按钮 |
| CHAT-10 | 会话列表：按项目分组，显示 Agent 图标、运行状态点、最后活动时间；支持改名、删除、搜索 | P0 | — |
| CHAT-11 | 打开会话时按页加载，向上滚动加载更早的内容；用户上滚时停止自动跟随，显示“回到最新” | P0 | 1 万条事件的会话首屏打开时间 < 1 秒 |
| CHAT-12 | 状态栏：模型、审批模式、上下文占用（有真实读数时才显示）、执行目标（服务器 / 设备 + 路径） | P0 | 执行目标始终可见 |
| CHAT-13 | 追加输入、切换模型和推理强度、压缩、分叉、编辑重发 | P1 | 按能力矩阵显示 |
| CHAT-14 | 原生记录缺失时，明确显示错误和恢复建议，不悄悄新开会话 | P0 | — |
| CHAT-15 | 消息状态：发送中 / 已送达 / 状态未知（断线时）；状态未知的消息不自动重发 | P0 | AC10 |
| CHAT-16 | “打开原生终端”：在同一项目下打开 claude 或 codex 交互终端，并标注“这是独立的原生会话” | P0 | — |

### 9.5 终端（TERM）

| ID | 需求 | 优先级 | 验收 |
| --- | --- | --- | --- |
| TERM-01 | 远端 PTY：Shell / Claude / Codex，在项目目录下通过登录 shell 启动 | P0 | PATH 中能找到通过 nvm 安装的命令 |
| TERM-02 | 本机 PTY（本机项目）：pwsh / cmd / 用户配置的 shell | P0 | — |
| TERM-03 | xterm.js：Unicode、CJK 字宽、ANSI 颜色、bracketed paste、链接识别、搜索、复制 | P0 | 中文输入法候选框位置正确 |
| TERM-04 | 多标签和最多四分屏（左右 → 左一右二 → 2×2），分屏比例可以拖动调整 | P0 | — |
| TERM-05 | 窗口缩放时防抖（100ms）后同步终端尺寸 | P0 | — |
| TERM-06 | 回滚缓冲默认 10000 行，可以配置；输出用批量帧推送，前端用 rAF 合并写入 | P0 | 性能见第 13 节 |
| TERM-07 | 快捷键：终端获得焦点时，除已声明的全局键外，所有按键都发给 PTY；冲突可以在设置中查看和修改 | P0 | Ctrl+C / Ctrl+R / Alt 组合在 CLI 中正常工作 |
| TERM-08 | 终端保活（可选使用 tmux） | P2 | — |

### 9.6 文件（FILE）

| ID | 需求 | 优先级 | 验收 |
| --- | --- | --- | --- |
| FILE-01 | 文件树：懒加载、刷新、显示隐藏文件开关、软链接标识 | P0 | — |
| FILE-02 | 右键菜单：新建文件或目录、重命名、删除（确认框显示完整路径）、复制路径、在终端中打开 | P0 | — |
| FILE-03 | 下载文件或目录（目录先在远端打包成 tar.gz，下载后删除远端临时文件），保存位置由用户选择，重名时自动加序号 | P0 | — |
| FILE-04 | 上传：拖拽或选择本机文件，传到远端当前目录 | P1 | 覆盖前确认 |
| FILE-05 | 传输队列：进度、速度、取消、失败重试 | P0 | — |
| FILE-06 | 编辑器（CodeMirror 6）：语法高亮、搜索替换、撤销、未保存标识；UTF-8，2 MiB 上限 | P0 | 保留原文件的 CRLF 换行和权限 |
| FILE-07 | 保存前比较 mtime 和大小，发现外部修改时提示：重新加载 / 查看 diff / 强制覆盖 | P0 | — |
| FILE-08 | 保存失败时内容留在编辑器里，可以另存到本机 | P0 | — |

### 9.7 后台、通知与系统（SYS）

| ID | 需求 | 优先级 | 验收 |
| --- | --- | --- | --- |
| SYS-01 | 关闭窗口时选择最小化到托盘或退出；托盘图标显示是否有运行中的任务 | P0 | 托盘菜单可以打开主窗口、退出 |
| SYS-02 | 系统通知：运行完成、等待审批、连接断开（每类可以单独关闭） | P0 | 点击通知跳到对应会话 |
| SYS-08 | 界面文案全部走 vue-i18n（zh-CN 默认、en），设置中可以切换语言 | P0 | 两种语言的键完全对齐，没有空翻译（沿用 pymss 的 `i18nKeys.test.ts`） |
| SYS-03 | 退出确认：列出未保存的文件、运行中的任务（任务在服务器上继续运行）、会被关闭的终端 | P0 | AC13 |
| SYS-04 | 单实例运行（沿用 pymss 的 `single_instance.rs`） | P0 | — |
| SYS-05 | 客户端自动更新（沿用 pymss 的 updater 体系） | P1 | — |
| SYS-06 | 诊断：连接各阶段的耗时和失败点、导出脱敏日志包 | P0 | 导出内容中没有秘密和项目内容 |
| SYS-07 | 命令面板（Ctrl+K）：切换项目或会话、新建终端、执行常用命令 | P1 | — |

---

## 10 前端设计

### 10.1 外壳

完全沿用 pymss 的外壳：`TitleBar.vue`（40px）+ `SideNav.vue`（64px）+ `<router-view>`，以及 `global.scss`、`theme.ts`、MiSans 字体、Naive UI 主题注入。尺寸与配色以 GPT 版的[布局规范](./前端布局与交互规范.md)为准。

**可以直接改造复用的 pymss 文件**（只复用结构，不复制业务）：`App.vue`、`TitleBar.vue`、`SideNav.vue`、`styles/global.scss`、`utils/theme.ts`、`utils/appZoom.ts`、`stores/toast.ts`、`stores/settings.ts` 的设置读写模式、`i18n/index.ts`、`stores/update.ts`，以及 Rust 侧的 `single_instance.rs`、`session_log.rs`、`storage.rs`（数据目录优先级）、`update_*`。

### 10.2 路由

| 路由 | 导航 | 页面 |
| --- | --- | --- |
| `/workbench/:projectId?/:sessionId?` | 工作台 | 默认页 |
| `/connections` | 连接 | 列表 + 新增向导 |
| `/projects` | 项目 | 网格或列表，按服务器和设备分组 |
| `/tasks` | 任务 | 安装、传输（以后加定时任务） |
| `/environment/:connectionId?` | 环境 | Host 和 CLI 状态、安装、登录 |
| `/diagnostics` | 诊断（放在导航底部） | 连接阶段时间线、日志 |
| `/settings` | 设置（放在导航底部） | — |

### 10.3 工作台线框

```text
┌ 标题栏：AgentDesk · 工作台                    ●prod-sg  ⇅ 已连接   ─ □ × ┐
├──┬─────────────┬───────────────────────────────────┬────────────────┤
│  │ 🔍 搜索       │ my-api · Codex · gpt-5           │ 文件  Git  详情 │
│导│ ★ my-api     │ 🖥 prod-sg:/home/u/my-api   [终端][⋯]│ ▸ src/          │
│航│   ● 修复登录   │───────────────────────────────────│ ▸ tests/        │
│  │   ○ 重构缓存   │  用户：修一下登录接口的 500          │   README.md     │
│  │ ▸ web-client │  ▸ 思考摘要（已折叠）                 │                 │
│  │ ── 本机 ──   │  ▾ 工作步骤 · 4 个工具 · 12s          │                 │
│  │ ▸ game-proj  │    ✓ 读取 src/auth.ts               │                 │
│  │              │    ✓ 运行 pnpm test  (exit 1)       │                 │
│  │ [+ 新建会话]  │  ┌ 需要审批 ─────────────────┐       │                 │
│  │              │  │ 执行：rm -rf dist          │       │                 │
│  │              │  │ [拒绝]  [批准]  [本会话总是]  │       │                 │
│  │              │  └────────────────────────────┘       │                 │
│  │              │───────────────────────────────────│                 │
│  │              │ ┌───────────────────────────────┐ │                 │
│  │              │ │ 输入消息…                      │ │                 │
│  │              │ └───────────────────────────────┘ │                 │
│  │              │ gpt-5 ▾  审批:按需 ▾  上下文 41%  [■停止] │                 │
│  ├─────────────┴───────────────────────────────────┴────────────────┤
│  │ 底部抽屉：终端标签 [bash ×][codex ×][+]          [⤢ 专注视图]       │
└──┴──────────────────────────────────────────────────────────────────┘
```

- 三栏都可以折叠；窗口宽度小于 1180px 时，右栏自动收起成抽屉。
- “专注视图”把终端区域提升为整页（标签 + 四分屏）；按 Esc 或点击按钮返回聊天。
- 编辑器标签和终端标签共用同一个标签系统（沿用 RemoteShell 的设计），可以在底部抽屉或专注视图中打开。

### 10.4 组件树（主要部分）

```text
views/WorkbenchView.vue
├─ workbench/ProjectSessionPanel.vue      项目与会话列表
├─ workbench/ChatHeader.vue               当前项目、Agent、执行目标、状态
├─ chat/MessageList.vue                   虚拟滚动（vue-virtual-scroller 或自己实现）
│   ├─ chat/UserMessage.vue
│   ├─ chat/AssistantText.vue             markdown-it + shiki（懒加载）
│   ├─ chat/ReasoningBlock.vue
│   ├─ chat/ToolStepGroup.vue → ToolCallItem.vue → ToolResultView.vue / DiffView.vue
│   ├─ chat/ApprovalCard.vue
│   └─ chat/QuestionCard.vue
├─ chat/Composer.vue                      输入框 + ModelPicker + PermissionPicker + 发送/停止
├─ workbench/SidePanel.vue                文件树 / Git / 详情
│   └─ files/FileTree.vue
└─ workbench/BottomDock.vue / FocusDock.vue
    ├─ tabs/TabStrip.vue
    ├─ tabs/SplitLayout.vue               最多 4 格
    ├─ terminal/TerminalView.vue          xterm.js + fit + search + web-links
    └─ editor/CodeEditor.vue              CodeMirror 6
```

### 10.5 Pinia stores

| store | 职责 |
| --- | --- |
| `connections` | 连接配置、实时状态（ssh / host / tunnel 分开记录） |
| `projects` | 合并远端 Host 的项目索引和本机项目 |
| `sessions` | 会话列表、当前会话、按会话维护事件缓存（有上限） |
| `runs` | 运行状态、待处理的审批 |
| `terminals` | PTY 标签、分屏布局 |
| `editors` | 打开的文件、修改状态 |
| `transfers` | 传输队列 |
| `env` | 每台服务器的环境检测结果 |
| `settings` / `toast` / `update` | 沿用 pymss |

**防串号规则**：所有异步响应都携带 `(connectionId, projectId, sessionId, requestSeq)`，store 处理前先核对当前目标是否一致，不一致就丢弃（落实 GPT 版 AC08）。

---

## 11 Tauri 后端设计

### 11.1 Rust 模块

```text
src-tauri/src/
├─ main.rs                  command 注册、插件
├─ state.rs                 AppState
├─ storage.rs               数据目录（沿用 pymss 的优先级逻辑）
├─ db/                      desktop.sqlite（rusqlite + 迁移）
├─ secrets.rs               keyring 封装
├─ ssh/
│   ├─ manager.rs           连接池、重连、心跳
│   ├─ auth.rs              密码 / 密钥 / agent / Pageant
│   ├─ known_hosts.rs
│   ├─ config_import.rs     解析 ~/.ssh/config
│   ├─ pty.rs               远端 PTY
│   ├─ sftp.rs              russh-sftp
│   └─ forward.rs           streamlocal 正向和反向转发
├─ host/
│   ├─ installer.rs         探测、安装方案、上传、升级
│   ├─ client.rs            JSON-RPC 客户端、订阅、补取、幂等
│   └─ protocol.rs          与 Python 共享的 schema（由 JSON Schema 生成）
├─ local/
│   ├─ pty.rs               portable-pty（ConPTY）
│   ├─ fs.rs                本机项目的文件操作
│   └─ executor/            回环 sshd、pwsh、Job Object；按平台分 mod
├─ platform/{windows,macos,linux}.rs
└─ commands/{conn,env,project,session,term,file,transfer,sys}.rs
```

### 11.2 Command 清单（前端 → Rust）

| 域 | command |
| --- | --- |
| conn | `conn_list` `conn_save` `conn_delete` `conn_import_ssh_config` `conn_connect` `conn_disconnect` `conn_trust_host_key` |
| env | `env_probe` `env_install_plan` `env_install_run` `env_install_cancel` `agent_login_start` |
| project | `project_list` `project_save` `project_archive` `dir_complete`（路径补全）|
| session | `session_list` `session_create` `session_history` `session_rename` `session_delete` `session_subscribe` |
| run | `run_send` `run_append` `run_stop` `approval_respond` `run_set_option` |
| term | `term_open` `term_write` `term_resize` `term_close` |
| file | `fs_list` `fs_read` `fs_write` `fs_mkdir` `fs_rename` `fs_delete` `fs_stat` |
| transfer | `transfer_download` `transfer_upload` `transfer_cancel` |
| sys | `diag_export` `open_log_dir` `settings_get` `settings_set` |

所有涉及目标的 command，第一个参数都是 `Target { connectionId? , deviceLocal?: bool, projectId? }`，由 Rust 决定走 SFTP 还是本机文件系统。前端不需要知道项目在哪。

### 11.3 事件推送

- 高频流（终端输出、会话事件）用 **`tauri::ipc::Channel`**，每个终端或订阅一条通道，按 16ms 或 32KB 批量发送。
- 低频状态（连接状态、传输进度、Toast）用 `app.emit`。

---

## 12 安全与威胁模型

| 资产 / 威胁 | 措施 |
| --- | --- |
| SSH 密码和密钥密码短语 | 存放在系统凭据库；默认不保存；内存中用后清零（zeroize） |
| 中间人攻击 | 严格校验主机指纹，指纹变化时拒绝连接，不提供“跳过验证” |
| 服务器上的其他用户连接 Host 或执行通道 | Unix socket 0600 + 目录 0700；执行通道再加一层临时密钥 SSH 认证 |
| 服务器被攻陷 / 同用户程序 | A-03：安全边界是服务器上的这个 Linux 用户，该用户下任何程序在通道开启期间都能连接本机。项目授权用于防误用，不对抗恶意程序隔离；首次启用如实告知，Host 强制检查并支持单项目撤销，断开失效 |
| Agent 越权读写 | 不承诺沙箱；审批沿用原生机制；bypass 模式必须在设置里打开并二次确认 |
| 日志泄露 | 统一脱敏（密码、令牌、私钥、授权链接中的 code）；终端内容和对话正文不写入日志 |
| 临时私钥泄露 | 不写入提示词、事件或日志；每次连接更换；仅在线且至少一个本机项目已授权时存在，最后一个项目撤销或断开立即删除 |
| 供应链 | 随包分发的 OpenSSH 和 pwsh 校验 SHA256；Host wheel 由 CI 构建并附带校验值；锁定依赖版本 |
| 账号合规 | 只使用官方 CLI 和官方登录；不代为登录，不共享账号；README 中说明（参照 CC Desk 的“账号须知”） |

---

## 13 非功能参数

| 项 | 目标 |
| --- | --- |
| 冷启动到显示界面 | < 2 秒（不等待网络） |
| 连接到 Host 就绪（已安装） | < 3 秒（局域网 / 同区域） |
| 会话首屏 | 1 万条事件的会话 < 1 秒（分页 + 虚拟滚动） |
| 流式渲染 | Markdown 增量渲染，每帧最多一次 DOM 更新 |
| 终端吞吐 | 四分屏、合计 200 行/秒、每行约 100 字符，持续 10 分钟，界面不卡顿，内存不持续增长 |
| 内存 | 空闲 < 300 MB；每多一个终端增加 < 30 MB（以 10000 行回滚计） |
| Host 资源 | 空闲 RSS < 80 MB；可以在 1 核 1G 的服务器上运行 |
| 事件缓存 | 桌面每个会话最多缓存 2000 条，超出后按页丢弃 |
| 平台 | Windows 10 21H2+ / 11 x64 首发；服务器 Linux x86_64 和 aarch64，glibc ≥ 2.28（musl 系统为 P2） |
| 国际化 | 全部文案走 vue-i18n，首发 zh-CN（默认）和 en（D-07） |
| 无障碍 | 按 GPT 版布局规范第 7 节 |

---

## 14 工程结构与技术选型

### 14.1 仓库结构（单仓库）

```text
agentdesk/
├─ src/                    Vue 前端（目录结构对齐 pymss）
├─ src-tauri/              Rust
├─ host/                   Python Agent Host（pyproject + uv）
│   ├─ agentdesk_host/{server,rpc,session,events,adapters/{claude,codex},device,store}
│   └─ tests/
├─ protocol/               JSON Schema → 生成 TS 类型、Rust 结构体、Python 模型
├─ resources/windows/      OpenSSH、pwsh（构建时下载并校验，不提交进仓库）
├─ scripts/                构建、打包、版本同步
├─ docs/
└─ .github/workflows/      CI：前端、Rust、Python、protocol 一致性、密钥扫描、打包
```

### 14.2 选型

| 用途 | 推荐 | 备注 |
| --- | --- | --- |
| 前端基础 | Vue 3.5 + TS + Vite 7 + Naive UI + Pinia 3 + Vue Router 4 + vue-i18n 11 + SCSS + pnpm | 与 pymss 一致 |
| 终端 | `@xterm/xterm` + addon-fit / search / web-links / unicode11；可选 webgl 渲染 | WebView2 中的输入法要做 Spike |
| 编辑器 | CodeMirror 6 | 比 Monaco 轻 |
| Markdown | markdown-it + shiki（懒加载语言包）+ DOMPurify | — |
| 虚拟列表 | vue-virtual-scroller，或自己实现（高度不定的列表） | — |
| SSH | `russh` + `russh-sftp` | 需要验证 streamlocal 反向转发和 Pageant 支持 |
| 本机 PTY | `portable-pty` | ConPTY |
| 本机存储 | `rusqlite`（bundled）+ `refinery` 迁移 | — |
| 秘密 | `keyring` | — |
| Host | Python 3.12 + asyncio + `claude-agent-sdk` + `aiosqlite` + `pydantic` | uv 打包 |
| 协议代码生成 | JSON Schema → `json-schema-to-typescript`、`typify`、`datamodel-code-generator` | 三端类型一致 |
| 测试 | 前端 node:test + happy-dom（沿用 pymss）；Rust cargo test；Python pytest；E2E 用 Playwright + 模拟 Host | 模拟 Host 让开发时不需要真实账号（参考 CC Desk 的 `npm run dev`） |

---

## 15 里程碑与 Spike

### 15.1 Spike（先验证最大的风险，每项都有明确的通过标准）

| ID | 内容 | 通过标准 |
| --- | --- | --- |
| S1 | russh：密码 / 密钥 / Pageant 认证、PTY、SFTP、direct-streamlocal、**streamlocal 反向转发** | 在 Windows 上对 Ubuntu 22.04 全部跑通；断网后 45 秒内检测到断线 |
| S2 | WebView2 + xterm.js 运行真实的 claude 和 codex CLI | 中文输入法、粘贴多行、缩放、Ctrl 组合键、四分屏 200 行/秒都没有问题 |
| S3 | Codex app-server 驱动：thread / turn / 审批 / 中断 / resume；**本机模式下由桌面下发的沙箱权限能否放行 ssh** | 4 种组合中的 Codex 两种跑通；确定 6.4 节的权限档位（优先第 1 档），以及用 `-c` 还是 `thread/start` 参数下发 |
| S4 | Claude Agent SDK（Python）：流式、can_use_tool、AskUserQuestion、interrupt、resume、原生记录检查 | Claude 两种组合跑通；重启 Host 后能准确恢复上下文（参照 CC Desk 的实测方法） |
| S5 | 反向通道：回环 sshd + pwsh `device-script` 在 Windows 上执行 | 跨行脚本、UTF-8、退出码、不弹窗、Job Object 回收都通过；被 Defender 拦截时能给出明确提示 |
| S6 | Host 部署：uv 在 CentOS 7 / Ubuntu 20.04 / Debian 12 / aarch64 上安装；有 systemd --user 和没有两种情况 | 不使用 sudo 就能装好；断开 SSH 后 Host 继续运行 |

### 15.2 里程碑

| 里程碑 | 内容 | 完成标志 |
| --- | --- | --- |
| M0 工程骨架 | 外壳 + 路由 + 主题 + 设置 + 单实例 + CI，协议 schema 和代码生成 | `pnpm tauri dev` 可以运行，CI 通过 |
| M1 SSH 工作台 | 连接（三种认证、导入、指纹）、文件树、编辑器、终端（多标签、四分屏）、传输 | 不依赖 Host 也能完整替代 RemoteShell |
| M2 Host + 远端图形对话 | Host 安装和升级、协议、Claude 和 Codex 适配器、聊天界面、审批、停止、补取 | 能力矩阵的“远端项目”一列全部 P0 通过 |
| M3 本机模式 | 执行通道、本机项目、本机终端和文件、设备入口和宽限期 | 能力矩阵的“本机项目”一列全部 P0 通过 |
| M4 首个公开版 v0.1 | 项目管理补全、诊断、托盘和系统通知、英文翻译补全、安装包、文档、发布校验 | GPT 版 AC01–AC19 全部通过 |
| M5 v0.2+ | P1：追加输入、模型和推理控制、压缩、分叉、diff、上传、Git、多设备接手、自动更新 | — |
| M6 | P2：用量统计、定时任务、WSS 和浏览器入口、macOS / Linux 客户端、worktree | — |

M1 是一个可以单独使用的版本，用户可以先拿它替代 RemoteShell，降低整体交付风险。

---

## 16 风险登记

| 风险 | 可能性 | 影响 | 应对 |
| --- | --- | --- | --- |
| Codex 的权限参数或 app-server 接口变化 | 中 | 中 | 权限由桌面启动时下发（D-05），S3 确定档位；锁定 CLI 的最低版本；适配器负责隔离接口变化 |
| Claude SDK 或 CLI 的控制协议变化 | 中 | 中 | 使用官方 SDK，不自己实现协议；锁定版本，CI 中跑适配器契约测试 |
| russh 的反向 streamlocal 或 Pageant 支持不完整 | 中 | 中 | S1 验证；备选方案是 TCP 反向转发绑定 127.0.0.1 再加临时密钥认证 |
| WebView2 中 xterm 的输入法和快捷键问题 | 中 | 中 | S2 验证；备选是 webgl / canvas 渲染器，或在 Rust 侧拦截按键 |
| 随包的 sshd 和 pwsh 被安全软件拦截 | 中 | 中 | 校验值 + 明确提示（参照 CC Desk）；备选方案是 Rust 内嵌 SSH 服务端（P-05） |
| 服务器的 glibc 太旧或架构不常见 | 低 | 中 | uv 的 python-build-standalone 支持 glibc 2.17+；aarch64 列入 CI |
| 账号风控或条款问题 | 低 | 高 | 只用官方 CLI 和官方登录；README 中写明边界；不提供共享功能 |
| 首版范围过大 | 高 | 高 | 按 M1 → M2 → M3 拆分，每个里程碑单独可用；P1 和 P2 严格不进入 v0.1 |

---

## 17 还需要确认的问题

下面这些不影响开始 M0 和 M1，可以在推进过程中逐步确定：

1. **正式产品名与仓库地址**（D-04）。
已解决：许可证定为 AGPL-3.0（D-06）；Codex 本机模式的权限由桌面启动时下发（D-05）；界面走 i18n、首发附带英文（D-07）；托盘和通知进入 v0.1（D-08）；不做离线安装包（D-09）。
