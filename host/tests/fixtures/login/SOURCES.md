# 登录输出样例来源

- `claude-waiting.txt`：2026-10-26，专用服务器用户 `agentdesk-test`，Claude Agent SDK 0.2.174 所带官方 CLI 2.1.310；运行 `claude auth login`，获得链接及回填提示后立即终止进程组。OSC 8 保留用于清洗回归，授权链接和 state 均已替换为假值。
- `codex-waiting.txt`：同日同一测试身份，官方 Codex 0.164.0；运行 `codex login --device-auth`，获得设备码后立即终止进程组。URL 和设备码均已替换为假值，保留 ANSI 颜色控制符。另实测 app-server `account/login/start {type:chatgptDeviceCode}` 返回 `loginId/verificationUrl/userCode`，随后立即调用 `account/login/cancel` 并关闭 app-server。
- Claude 成功模板 `Login successful`：官方认证文档 <https://code.claude.com/docs/en/authentication>。失败模板 `Login failed: `、`Authentication failed: Invalid authorization code` 及 `Token exchange failed (...)`：只读检索测试服务器官方 bundled CLI 2.1.310 字符串与邻近登录分支，未执行授权。
- Codex 成功模板 `Successfully logged in` 与失败前缀 `Error logging in with device code: `：官方固定版本源码 <https://github.com/openai/codex/blob/rust-v0.164.0/codex-rs/cli/src/login.rs>。过期模板 `device auth timed out after 15 minutes`：<https://github.com/openai/codex/blob/rust-v0.164.0/codex-rs/login/src/device_code_auth.rs>。

`test_login.py` 的其余设备码、回填、失败及 Claude 过期文本为明确标记的构造健壮性用例，不能视为实际授权录制。没有打开浏览器、完成授权、使用账号、读取或复制凭据。成功状态最终由退出后的官方状态检测确认，不能仅凭输出模板判定。

录制文件统一为 LF，并去除行尾空白和末尾空行；保留原文与 ANSI/OSC 控制序列，不改写提示内容。
