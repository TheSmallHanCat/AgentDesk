# 用户本人登录 Claude / Codex

AgentDesk 不代为登录，不收集或复制账号凭据。请先在环境页完成 Host 安装与启动；Fake Agent 不需要登录。

## 通过桌面登录

1. 连接测试服务器，打开「环境」，选择该连接并刷新检测结果。
2. 在 Claude / Codex 的状态旁点击「登录」。桌面会打开远端 PTY，执行官方登录命令。
3. 点击终端中的授权链接，在浏览器由本人完成授权；需要输入代码时粘贴到该终端。
4. 返回环境页刷新，确认对应 Agent 显示已登录。模型列表来自原生 CLI，是否能使用模型仍需真实运行验证。

登录终端可以关闭；Host 和服务器上的原生会话独立存活。不要把授权代码、链接中的敏感参数、令牌或账号目录发送给架构师。

## 手工 SSH 备用方式

用原有连接信息，以 `agentdesk-test` 登录服务器（替换尖括号内容）：

```sh
ssh -p <SSH端口> agentdesk-test@<服务器>
```

安装后的固定入口如下，无需改 PATH：

```sh
# Claude：定位当前 Host 虚拟环境随 SDK 安装的官方 CLI
agentdesk_python="$HOME/.agentdesk/host/current/venv/bin/python"
agentdesk_claude="$("$agentdesk_python" -c 'import pathlib,claude_agent_sdk; print(pathlib.Path(claude_agent_sdk.__file__).parent / "_bundled" / "claude")')"
"$agentdesk_claude" auth login
"$agentdesk_claude" auth status

# Codex：当前 Host 版本配套的官方二进制
"$HOME/.agentdesk/host/current/bin/codex" login --device-auth
"$HOME/.agentdesk/host/current/bin/codex" login status
```

测试用户最终路径为 `/home/agentdesk-test/.agentdesk/host/current/`；`current` 指向已校验的版本目录。不要使用任务 02 的临时 Spike 路径。登录状态由官方 CLI 自行保存，桌面只查询是否登录及账号类型。

## 登录后通知架构师

只需告知：“测试用户的 Claude / Codex 官方登录已完成，请执行任务 03 的真实 Agent 验收。”无需提供账号或登录输出。

待验收：两种 Agent 的新建、模型可用性、流式文本、工具与文件事件、审批允许/拒绝、多设备竞争、AskUserQuestion、停止、断线补取、原生恢复、原生记录缺失错误。Codex 还需在真实模型流程中复核权限实际生效。自动契约入口：在服务器的隔离测试目录显式设置 `AGENTDESK_NATIVE_CONTRACT=1` 后执行 Host pytest；该步骤会发送模型请求，应在架构师组织验收时执行。

测试服务器低于 glibc 2.28 的产品支持基线；本轮实测可运行不代表扩大支持范围。系统升级或 CLI 版本变化后需要重新检测。
