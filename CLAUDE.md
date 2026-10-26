# 工程说明

先阅读 `AGENTS.md`、`README.md`、`docs/IMPLEMENTATION-CONTRACT.md` 和 `docs/progress/01-M0-M1.md`。

主设计文档为上级 `docs/远程开发工作台需求设计-Claude版.md`，第 1 节决策记录优先。参考工程只读。M1 保留不依赖 Host 的 SSH 工作台；M2 增加远端 Host、官方适配器与图形会话，接口见 `docs/M2-CONTRACT.md`，真实模型验收须在用户本人登录后单独执行。本机项目与执行通道属于 M3。不得通过终端抓屏伪造 Agent 图形会话，也不得用桌面显示历史重建原生上下文。

新增阶段请明确需求编号、目标隔离、秘密存储、资源释放和实际测试结果，保留已有行为保证。
