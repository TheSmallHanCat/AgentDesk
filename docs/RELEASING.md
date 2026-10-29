# GitHub 发布准备

公开源码与分发桌面安装包是两个独立步骤。仓库中的 Windows 工作流按需构建未签名安装包并上传 Actions artifact，不自动创建 GitHub Release。

## 源码范围

提交产品源码、测试夹具、协议、文档、锁文件和构建脚本。以下内容应留在本机：

- `ssh.txt`、`.env`、私钥、凭据、数据库和账号配置。
- `.agentdesk-data/`、`.tmp/`、`test-results/` 及工具缓存。
- `node_modules/`、`dist/`、Rust target 和下载生成的发行资源。
- 包含个人部署目录、账号状态或维护信息的运维记录。

`.gitignore` 防止未跟踪文件被普通 Git 操作加入，但不会移除已经提交的内容。发布前检查候选文件、提交差异和已有 Git 历史。`scripts/check_repository.py` 检查常见私有文件、构建产物和文档链接；CI 中的 Gitleaks 另负责秘密扫描，二者不能互相替代。

```bash
python scripts/check_repository.py
git status --short
git diff --check
pnpm test
pnpm typecheck
pnpm format:check
pnpm build
pnpm protocol:check
uv run --project host python -m pytest host/tests -q
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --locked --manifest-path src-tauri/Cargo.toml
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

真实服务器验收不在普通 CI 中执行，不应向 pull request 工作流注入个人服务器密码或正式账号令牌。其范围见 [验收说明](../scripts/e2e/README.md)。

## Windows 构建

```bash
python scripts/build/build_host_resources.py
python scripts/build/build_windows_resources.py
pnpm tauri build
```

构建入口使用锁定依赖和经过摘要核验的官方运行资源。执行 `.github/workflows/package.yml` 可取得安装包和 `SHA256SUMS.txt`；当前产物未签名，发布说明应如实标明。

版本分别位于 `package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json` 与 `host/pyproject.toml`。桌面和 Host 版本可以不同，但一次发布应说明对应关系及数据库兼容性。

保留 [LICENSE](../LICENSE) 与 [NOTICE](../NOTICE)。分发安装包前，按实际打包的依赖补齐第三方许可文本和版权清单；现有 NOTICE 仅列主要组件。

## 发布说明

说明支持的平台、实际验证过的服务器条件、尚未完成的实机项目，以及升级和回退方式。官方账号登录、模型任务、系统缩放或输入法等未执行的检查，不应写为已通过。

Host 数据库升级前保留备份。切换版本链接不会自动回退数据库；恢复旧二进制前需确认 schema 兼容。不要将历史测试服务器的结果泛化为所有环境均受支持。
