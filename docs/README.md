# 文档导航

## 使用与开发

- [开发、使用与运行边界](DEVELOPMENT.md)：工具链、运行命令、界面操作、数据位置及权限边界。
- [代码结构与运行流程](ARCHITECTURE.md)：模块职责、调用方向和修改入口。
- [官方 Agent 登录](USER-LOGIN.md)：图形授权流程及待验证事项。
- [发布准备](RELEASING.md)：源码检查、Windows 构建和公开仓库范围。
- [脚本导航](../scripts/README.md)：构建、浏览器验证及真实环境验收入口。

## 接口契约

文档中的 M0/M1、M2、M3 为实现阶段编号，保留名称便于关联历史决策。

- [桌面 IPC 与目标归属](IMPLEMENTATION-CONTRACT.md)
- [Host RPC、会话、事件与登录](M2-CONTRACT.md)
- [可选本机执行通道](M3-CONTRACT.md)
- [可选服务器隔离工作区](ISOLATION-CONTRACT.md)

普通远程模式不需要容器工作区；本机执行通道也需另行授权。

## 设计与历史

- [主设计与决策记录](design/远程开发工作台需求设计-Claude版.md)
- [需求文档](design/远程开发工作台需求文档.md)
- [界面与交互规范](design/前端布局与交互规范.md)
- [当前决策与待确认事项](QUESTIONS.md)
- [历史决策记录](archive/QUESTIONS-M0-M3.md)
- [阶段报告归档](archive/progress/README.md)

设计文档中的待评审方案不等同于已经实现。实际接口以契约和代码为依据，历史报告仅记录对应阶段的验收结果。个人服务器部署记录、连接配置和运行日志不属于公开文档。
