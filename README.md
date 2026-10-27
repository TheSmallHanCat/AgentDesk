# AgentDesk

Windows 优先的 SSH 远程开发工作台，采用 Vue 3、TypeScript、Naive UI 与 Tauri 2。项目采用 **AGPL-3.0-only** 许可证。

当前实现包括 M0/M1 SSH 工作台与 M2 远端 Agent Host、图形会话、环境预检及安装。Fake Agent 可在未登录时验证完整流程；Claude/Codex 通过官方 CLI 工作，真实模型验收需要用户本人先登录。M3 支持 Windows 本机项目、ConPTY、本机文件编辑与反向 SSH 执行通道，启用前须按项目确认服务器执行权限。远端安装在环境页展示完整方案，经用户确认后执行。

## 开发

需要 Node.js 24、pnpm 10.33.2、Rust 1.96（MSVC 工具链、Windows SDK）、Python 3.12 和 uv；运行桌面需要 WebView2。

```bash
pnpm install --frozen-lockfile
pnpm dev
python scripts/build_windows_resources.py
pnpm tauri dev
```

`pnpm dev` 可在浏览器查看外壳和设置，SSH 与本机文件能力需要桌面程序。开发模式默认读取仓库内被 Git 忽略的 `.agentdesk-data/`。

```bash
pnpm test
pnpm typecheck
pnpm format:check
pnpm build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo build --manifest-path src-tauri/Cargo.toml
uv sync --project host --frozen
uv run --project host python -m pytest host/tests -q
pnpm protocol:check
```

协议类型变更后运行 `pnpm protocol:generate`。`protocol/*.schema.json` 是握手、M2 方法和事件类型的唯一来源，生成 TS、Rust serde 与 Python dataclass。生成器严格支持当前使用的 Schema 子集；类型本身不替代运行时校验。Host 使用 Python 3.12、Unix socket、NDJSON JSON-RPC、SQLite、官方 SDK/app-server 适配器。

## 使用

1. 在连接页添加地址、端口、用户与认证方式。地址支持 `user@host:port`、`[IPv6]:port`。也可导入 `~/.ssh/config`，不会回写该文件；Include/Match 会显示不支持说明，ProxyJump 只解析。
2. 首次连接通过可信渠道核对主机指纹。变化的指纹会拒绝连接，不提供跳过校验入口。密码与私钥密码短语默认不保存，选中后写入操作系统凭据库。
3. 在项目页选择服务器目录或本机设备目录。本机使用系统目录选择器；远端绝对目录支持路径补全与最近目录。目录不存在时确认创建；收藏、归档和恢复只操作索引。
4. 环境页先检测服务器，再展示安装计划、实时日志和取消操作。安装前运行 `python scripts/build_host_resources.py` 准备发行资源；Windows 打包 workflow 已纳入该步骤。CLI 不改 PATH 或已有配置，点击 Agent 旁的“登录”，在软件内转交官方授权链接、设备码或回填代码。开发时可在计划中显式启用 Fake Agent。图形会话与原生终端均可在工作台打开；原生终端为独立会话。
5. 右侧文件树支持懒加载、隐藏文件、软链接标识与右键菜单。双击文件进入编辑器；UTF-8 文本上限 2 MiB，支持撤销、搜索替换和常见语言高亮。
6. 终端与编辑器共用标签，可切换专注视图，最多四分屏；关闭分屏保留标签。关闭终端标签会结束对应 PTY，断线后不会自动重建终端。
7. 下载通过系统目录选择器确定本机位置；目录远端流式打包，重名自动编号。任务页显示进度、速度、取消及重试。退出前显示未保存文件、终端和传输。

默认全局快捷键：`Ctrl+Shift+T` 新建 Shell，`Ctrl+S` 保存当前编辑器，`Ctrl+Shift+F` 切换专注视图。可以在设置中查看和修改。其他终端按键交给远端程序。

`Ctrl+Shift+W` 关闭当前标签，`Ctrl+PageDown` / `Ctrl+PageUp` 切换标签，`Ctrl+Alt+S` 新增分屏。终端内保留 Esc 给原生程序；通过按钮或专注视图快捷键返回工作台。

已有项目关联的连接不能修改地址、端口和用户；需要更换目标时新增连接，避免已有编辑器被重定向。项目创建后的执行路径固定，其他显示和默认项可以修改。

## 结构与数据

```text
src/           Vue 页面、组件、Pinia 状态与本机 IPC
src-tauri/     Rust SSH、PTY、SFTP、SQLite、系统凭据与桌面生命周期
host/          Python 3.12 Host、SQLite、官方适配器与 Fake Agent
protocol/      JSON Schema、握手样例与三端类型生成
scripts/       检查与浏览器验证
docs/          接口约定、问题与进度报告
```

数据目录优先级：`AGENTDESK_DATA_ROOT` → 开发模式 `.agentdesk-data/` → 便携版可执行文件旁 `agentdesk.portable` 标记对应的 `data/` → 系统配置目录 `AgentDesk/`（Windows 为 `%APPDATA%/AgentDesk/`）。可以使用环境变量指定隔离测试目录，不要把个人路径写进代码。

连接和远端项目索引保存在 `desktop.sqlite`；设置为 `settings.json`；应用独立管理 `known_hosts` 并只读系统的 `~/.ssh/known_hosts`。M1 项目索引保留，M2 按原 id 幂等登记到 Host；会话与事件由 Host 保存，桌面仅存归属映射与草稿。不同连接的索引不自动合并。诊断日志有大小上限，仅含连接 ID、阶段、耗时和错误码，不记录凭据、终端输出或项目内容。凭据保存在系统 keyring，自动重连所需的临时凭据只在进程内存中保留，主动断开后释放。

初始交付见 [任务 01 报告](docs/progress/01-M0-M1.md)，审查修复与真实环境结果见 [任务 02 报告](docs/progress/02-fix-and-e2e.md)。M2 实测与限制见 [任务 03 报告](docs/progress/03-M2.md)。可重复验收入口见 [E2E 说明](scripts/E2E.md)。本地 SSH 测试服务只监听回环地址、每次生成临时主机密钥，不依赖用户账号。

浏览器验证脚本需要额外安装 Playwright 和 Chromium；启动 `pnpm dev --host 127.0.0.1` 后，可运行 `python scripts/smoke_ui.py` 与 `python scripts/smoke_workbench.py`。后者使用测试专属 IPC 数据，不连接外网。构建 Windows debug 程序后，`python scripts/smoke_native.py` 使用隔离数据目录验证真实 WebView 与 IPC；截图写入被忽略的 `test-results/`。日常 `pnpm test` 不需要 Playwright。

## 本机模式安全边界（A-03）

本机模式允许服务器以桌面当前 Windows 用户身份执行脚本，访问该用户有权访问的文件；它不是沙箱。安全边界是**服务器上的这个 Linux 用户**：以该用户运行的任何程序，在通道开启期间都能读取设备入口和临时密钥、连接本机。每项目授权用于防止误用，不是对抗恶意程序的隔离。只有信任该服务器及该 Linux 用户下运行的所有程序时才应启用。

启用任一项目，即允许该会话以当前 Windows 用户身份访问整台电脑中该用户有权访问的资源。项目路径只是工作目录约定，不是执行层的路径访问限制。

桌面和 Host 都检查当前连接的项目授权；工作台提供单项目撤销，并中断该项目正在运行的任务。最后一个项目撤销或设备断开时立即删除服务器设备密钥及该设备全部会话入口，断开后授权失效。重连后仅为本次已授权项目重建入口，其他项目的活动任务会被中断。若需要抵抗同用户恶意程序，需要另行采用每项目独立 Linux 用户等操作系统隔离措施。

本机项目的 Claude/Codex 均拒绝 `plan`：服务器进程的只读限制无法保证 SSH 通道中的 Windows 文件只读。Codex 常规本机执行仍仅使用 A-02 第 1 档，不能自动升级权限。

安装失败诊断额外保存退出码与 stderr 尾部中白名单允许的错误类别，其他内容替换为 `[redacted]`，不保存原始 stderr 或秘密。

## 账号与发布边界

Agent 能力使用用户服务器上未修改的官方 CLI 与官方登录，不收集或复制登录令牌，不在桌面直接请求模型服务。用户通过软件内登录对话框完成官方授权；验收脚本只获取登录提示并立即取消，不完成授权、不发送真实模型请求。当前不包含生产服务部署或自动更新。

Windows 首发；macOS/Linux 的平台代码已有隔离边界，尚未通过真实设备验收。源码构建通过不等同于已完成安装包、中文输入法、系统缩放或真实 CLI 性能验收。

M3 接口见 [本机执行契约](docs/M3-CONTRACT.md)，实测与回滚见 [任务 04 报告](docs/progress/04-fix-M3.md)。随包组件仅存 `resources/windows/`，由固定版本和 SHA256 校验脚本准备，不提交二进制。
