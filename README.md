# AgentDesk

Windows 优先的 SSH 远程开发工作台，采用 Vue 3、TypeScript、Naive UI 与 Tauri 2。项目采用 **AGPL-3.0-only** 许可证。

当前交付范围为 M0 工程骨架与 M1 SSH 工作台：连接、远端项目、终端、文件浏览、文本编辑和下载。图形 Agent 对话、Agent Host 业务和本机执行通道将在 M2/M3 实现。环境页和聊天区明确显示阶段说明，不自动安装任何远端软件。

## 开发

需要 Node.js 24、pnpm 10.33.2、Rust 1.96（MSVC 工具链、Windows SDK）、Python 3.12 和 uv；运行桌面需要 WebView2。

```bash
pnpm install --frozen-lockfile
pnpm dev
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

协议类型变更后运行 `pnpm protocol:generate`。`protocol/hello.schema.json` 是 hello 握手及样例类型的唯一来源，生成 TS、Rust serde 与 Python dataclass。生成器严格支持当前使用的 Schema 子集；类型本身不替代运行时 Schema 校验。Host 此阶段仅有空包边界与协议样例，没有运行服务或官方 CLI 适配器。

## 使用

1. 在连接页添加地址、端口、用户与认证方式。地址支持 `user@host:port`、`[IPv6]:port`。也可导入 `~/.ssh/config`，不会回写该文件；Include/Match 会显示不支持说明，ProxyJump 只解析。
2. 首次连接通过可信渠道核对主机指纹。变化的指纹会拒绝连接，不提供跳过校验入口。密码与私钥密码短语默认不保存，选中后写入操作系统凭据库。
3. 连接后在项目页选择远端绝对目录，可使用路径补全与最近目录。目录不存在时确认创建；收藏、归档和恢复只操作索引。
4. 工作台可以打开 Shell、Claude Code 或 Codex 原生终端。CLI 必须已经在远端安装并由用户自行登录；每个终端都是独立会话，不共享未来的图形会话上下文。
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
host/          Python 3.12 包骨架
protocol/      JSON Schema、握手样例与三端类型生成
scripts/       检查与浏览器验证
docs/          接口约定、问题与进度报告
```

数据目录优先级：`AGENTDESK_DATA_ROOT` → 开发模式 `.agentdesk-data/` → 便携版可执行文件旁 `agentdesk.portable` 标记对应的 `data/` → 系统配置目录 `AgentDesk/`（Windows 为 `%APPDATA%/AgentDesk/`）。可以使用环境变量指定隔离测试目录，不要把个人路径写进代码。

连接和远端项目索引保存在 `desktop.sqlite`；设置为 `settings.json`；应用独立管理 `known_hosts` 并只读系统的 `~/.ssh/known_hosts`。M1 的远端项目索引先保存在桌面，M2 再设计迁移到 Host。诊断日志有大小上限，仅含连接 ID、阶段、耗时和错误码，不记录凭据、终端输出或项目内容。凭据保存在系统 keyring，自动重连所需的临时凭据只在进程内存中保留，主动断开后释放。

初始交付见 [任务 01 报告](docs/progress/01-M0-M1.md)，审查修复与真实环境结果见 [任务 02 报告](docs/progress/02-fix-and-e2e.md)。可重复验收入口见 [E2E 说明](scripts/E2E.md)。本地 SSH 测试服务只监听回环地址、每次生成临时主机密钥，不依赖用户账号。

浏览器验证脚本需要额外安装 Playwright 和 Chromium；启动 `pnpm dev --host 127.0.0.1` 后，可运行 `python scripts/smoke_ui.py` 与 `python scripts/smoke_workbench.py`。后者使用测试专属 IPC 数据，不连接外网。构建 Windows debug 程序后，`python scripts/smoke_native.py` 使用隔离数据目录验证真实 WebView 与 IPC；截图写入被忽略的 `test-results/`。日常 `pnpm test` 不需要 Playwright。

## 账号与发布边界

后续 Agent 能力仅使用用户服务器上未修改的官方 CLI 与官方登录，不收集或复制 Agent 登录令牌，不在桌面直接请求模型服务。当前不包含真实第三方登录、远端部署、生产服务配置或自动更新。

Windows 首发；macOS/Linux 的平台代码已有隔离边界，尚未通过真实设备验收。源码构建通过不等同于已完成安装包、中文输入法、系统缩放或真实 CLI 性能验收。
