# AgentDesk

通过 SSH 使用服务器上的官方 Claude Code 和 Codex，在一个 Windows 桌面应用中管理项目、文件、终端与 Agent 会话。

主要使用方式：项目代码、工具链和 Agent 均放在 VPS，本机负责交互。普通远程模式无需容器或额外公网端口。项目采用 **AGPL-3.0-only** 许可证。

## 功能

- SSH 密码、私钥和 agent 认证，主机指纹校验，系统凭据存储。
- 远端项目、文件编辑、终端分屏、下载与传输管理。
- 官方 Agent 图形登录、流式会话、工具审批、停止与恢复。
- 可选的 Windows 本机项目执行通道，需逐项目授权。
- 可选的服务器容器工作区，独立于普通远程使用流程。

当前以 Windows 为主要验证平台。真实账号登录、模型任务及平台兼容性取决于实际环境；尚未完成的验收项见开发文档和阶段记录。

## 开发运行

需要 Node.js 24、pnpm 10.33.2、Rust 1.96 的 MSVC 工具链及 Windows SDK、Python 3.12、uv 和 WebView2。

```bash
pnpm install --frozen-lockfile
uv sync --project host --frozen
python scripts/build/build_host_resources.py
python scripts/build/build_windows_resources.py
pnpm tauri dev
```

`pnpm tauri dev` 会启动前端开发服务。仅预览浏览器界面时使用 `pnpm dev`；SSH 和桌面系统能力需要原生程序。下载生成的资源及开发数据均被 Git 忽略。

## 使用

1. 添加 SSH 连接，核对服务器主机指纹。
2. 在环境页检测并安装 Host，通过官方流程登录 Agent。
3. 新建远端项目，选择服务器目录和“服务器普通环境”。
4. 在工作台创建会话，使用聊天、文件和终端。

本机执行通道与容器工作区均为可选能力，不是上述流程的前置条件。官方授权页若在本机浏览器打开，浏览器仍直接访问授权网站。

## 仓库结构

```text
src/                Vue 界面、组件、状态和 IPC 调用
src-tauri/          Rust 桌面后端、SSH、文件与系统能力
host/               Python Host、官方 Agent 适配器和服务端状态
protocol/           JSON Schema 与三端类型生成
resources/          发行资源说明与本机构建输出
scripts/build/      发行资源构建
scripts/browser/    浏览器交互回归
scripts/e2e/        真实桌面与服务器验收
scripts/check_repository.py  公开文件与文档链接检查
tests/              前端单元测试与 IPC 夹具
docs/               开发说明、架构、契约与设计
docs/archive/       历史阶段报告
spikes/             独立技术实验，不进入产品构建
```

Rust 测试位于 `src-tauri/tests/`，Host 测试位于 `host/tests/`。协议生成文件随源码提交，下载的二进制和运行数据不提交。

## 文档与贡献

- [文档导航](docs/README.md)
- [开发、使用与运行边界](docs/DEVELOPMENT.md)
- [代码结构与运行流程](docs/ARCHITECTURE.md)
- [脚本入口](scripts/README.md)
- [贡献约定](CONTRIBUTING.md)
- [GitHub 发布准备](docs/RELEASING.md)

提交前运行与改动相关的检查；协议变更执行 `pnpm protocol:generate` 和 `pnpm protocol:check`。真实服务器验收需要单独配置与授权，不会在普通单元测试中执行。

许可证见 [LICENSE](LICENSE)，第三方组件说明见 [NOTICE](NOTICE)。
