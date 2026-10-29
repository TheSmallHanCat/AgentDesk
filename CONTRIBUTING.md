# 参与开发

请先阅读 [README](README.md)、[代码结构](docs/ARCHITECTURE.md) 与所修改模块的[接口契约](docs/README.md)。主设计第 1 节决策记录优先；历史进度报告保留在 `docs/archive/progress/`，不作为当前开发入口。

使用 pnpm 和仓库现有锁文件。代码标识符与注释使用英文，文档和 Conventional Commits 提交说明使用简体中文，例如 `fix: 修复迟到的目录响应覆盖当前项目`。

保持模块边界：Vue 负责展示，Rust 负责权限与目标检查、SSH 和桌面能力，平台实现只放 `platform/`。异步目标必须携带连接、项目、会话及请求序号；终端和编辑器始终保留创建时的目标。

提交前运行[开发文档](docs/DEVELOPMENT.md)中与改动相关的测试与检查，以及 `python scripts/check_repository.py`。修改协议必须重新生成三端类型并运行一致性检查。SSH 集成测试运行在本机回环内存服务中，不需要生产凭据；真实服务器测试应使用专用目录，执行删除、覆盖和部署前明确授权。

脚本分别放入 `scripts/build/`、`scripts/browser/` 和 `scripts/e2e/`；不要让普通检查隐式调用真实服务器。文档从 `docs/README.md` 建立导航，私人部署记录留在忽略目录。

不要提交个人路径、`.agentdesk-data/`、密钥、令牌、配置或会话。不得记录终端正文、项目文件和完整请求 payload。不要把未实现能力包装为成功；在进度报告中注明缺口、实际验证命令和手工验收项。
