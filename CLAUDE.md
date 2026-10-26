# 工程说明

先阅读 `AGENTS.md`、`README.md`、`docs/IMPLEMENTATION-CONTRACT.md` 和 `docs/progress/01-M0-M1.md`。

主设计文档为上级 `docs/远程开发工作台需求设计-Claude版.md`，第 1 节决策记录优先。参考工程只读。当前阶段以 Host 无依赖的 SSH 工作台为边界；后续设计不得通过终端抓屏伪造 Agent 图形会话，也不得用桌面显示历史重建原生上下文。

新增阶段请明确需求编号、目标隔离、秘密存储、资源释放和实际测试结果，保留已有行为保证。
