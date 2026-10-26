# AgentDesk 协作约定

- 默认使用简体中文沟通、文档和提交说明，代码标识与注释使用英文。
- 先读主设计与 `docs/IMPLEMENTATION-CONTRACT.md`。主设计第 1 节决策记录优先；不修改仓库外的参考工程或设计文档。
- 保持 Vue / Rust / Python / 协议边界。M0/M1 不实现 Agent Host 业务、CLI 适配器或本机执行通道。
- 先检查已有入口与生命周期，进行最小可回退改动；不引入无需求抽象，不放置无功能按钮、空实现或虚假成功状态。
- 所有目标操作必须检查 `(connectionId, projectId, sessionId, requestSeq)`；资源后续操作检查创建时归属。迟到响应不得改动新的选中目标。
- 秘密仅进系统凭据库；不记录项目内容或终端输出。SSH 主机指纹变化拒绝连接。平台相关代码放 `src-tauri/src/platform/`。
- 删除、迁移或覆盖用户数据，真实第三方登录、远端部署和架构大改前确认范围；当前任务已明确授权的本机工程实现与测试无需重复询问。
- 检查命令见 README：`pnpm test/typecheck/build`、Rust `fmt/test/clippy/build`、Host `pytest`、`pnpm protocol:check`。
- 可以对独立且有收益的模块使用当前环境支持的子 agent；不要写死模型。若子任务工具不可用，主 agent 继续处理并如实报告，不把未执行的检查写成通过。
- 提交前统一检查 diff 与验证结果。使用 Conventional Commits 和中文描述，小步形成可运行里程碑。

- M3 本机项目由 Rust 根据持久归属分派文件与终端；反向执行通道必须先探测成功、再按项目授权，断开后授权失效。真实会话使用归档语义；Codex 本机模式仅使用 A-02 第 1 档。
