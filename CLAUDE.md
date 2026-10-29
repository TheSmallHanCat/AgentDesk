# 工程说明

先阅读 `AGENTS.md`、`README.md`、`docs/ARCHITECTURE.md` 和 `docs/IMPLEMENTATION-CONTRACT.md`。历史报告集中在 `docs/archive/progress/`。

主设计文档为 `docs/design/远程开发工作台需求设计-Claude版.md`，第 1 节决策记录优先。历史参考工程仅说明设计来源，不是本仓库的构建依赖。M1 保留不依赖 Host 的 SSH 工作台；M2 增加远端 Host、官方适配器与图形会话，接口见 `docs/M2-CONTRACT.md`，真实模型验收须在用户本人登录后单独执行。M3 增加 Windows 本机项目、独立执行端和反向 streamlocal 通道，接口与验收见 `docs/M3-CONTRACT.md`、`docs/archive/progress/04-fix-M3.md`。不得通过终端抓屏伪造 Agent 图形会话，也不得用桌面显示历史重建原生上下文。

新增功能请明确当前需求、目标归属、秘密存储、资源释放和实际测试结果，保留已有行为保证。当前决策与未决事项见 `docs/QUESTIONS.md`，历史阶段记录不作为现行功能禁令。

真实会话按 A-01 归档，保留原生记录。本机 Codex 按 A-02 使用第 1 档沙箱；本机任务必须同时满足执行通道探测通过与当前项目在本次连接中已获授权。不得读取账号凭据或自动登录。
