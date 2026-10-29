## Windows x64

- `*-setup.exe`：NSIS 安装包，首次安装可能需要联网准备 WebView2。
- `*-portable.zip`：完整解压后运行 `agentdesk.exe`；系统须已安装 WebView2。
- `SHA256SUMS.txt`：以上两个文件的 SHA256 校验值。

当前程序未进行代码签名，Windows 可能显示未知发布者提示。

便携版数据保存在程序旁的 `data/` 目录；密码和私钥口令仍保存在 Windows 凭据管理器，不随目录迁移。安装版使用系统应用数据目录。

项目代码、编译工具和 Agent 可运行在远端 Linux 服务器，通过 SSH 使用。Host 数据库格式为 `3`，不支持历史格式自动迁移。

本次自动发布仅支持 Windows。macOS 工作流仅提供手动实验构建，尚未完成实机验证。
