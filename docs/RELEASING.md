# GitHub 发布准备

推送版本标签后，Windows 工作流自动运行工程检查、构建安装包与便携包并发布 GitHub Release。macOS 仅提供独立的手动实验构建，不参与正式发布。

## 标签自动发布

先将代码推送到 GitHub 默认分支，再给准备发布的提交打标签并推送：

```bash
git tag -a v0.1.0 -m "AgentDesk v0.1.0"
git push origin v0.1.0
```

标签必须是 `vMAJOR.MINOR.PATCH`，或带预发布标识，例如 `v0.2.0-rc.1`。预发布标签自动创建 GitHub Pre-release；数字段不能有多余前导零，不接受 `+build` 后缀。标签必须包含本次工作流和发布脚本。

[Windows 构建与发布](../.github/workflows/package.yml) 依次执行：

1. 验证标签格式，固定对应的 commit。
2. 复用 [工程检查](../.github/workflows/ci.yml)，运行前端、Rust、Host、协议、公开文件与秘密扫描。任何检查失败均停止发布。
3. 在 CI 工作区同步桌面版本，构建 Windows x64 NSIS 安装包和便携 ZIP，生成 `SHA256SUMS.txt`。
4. 再次核对标签未移动、验证资产摘要，在草稿中上传全部文件，成功后公开 Release，并附带自动生成的提交说明。

只有发布 job 拥有 `contents: write`，其余 job 只读源码。使用 GitHub 自动提供的 `GITHUB_TOKEN`，无需 PAT、服务器凭据或签名密钥。仓库必须启用 Actions；如果组织策略禁止 Release 写入，需要管理员允许该 job 的权限。

发布资产：

- `AgentDesk-<版本>-windows-x64-setup.exe`
- `AgentDesk-<版本>-windows-x64-portable.zip`
- `SHA256SUMS.txt`

便携包包含 Host、OpenSSH、PowerShell 与随包许可文件，通过资源清单逐项核验和收集。不会打包本机应用数据。完整解压到可写目录后运行 `agentdesk.exe`；系统需要 WebView2。便携版数据保存在程序旁的 `data/`，密码和私钥口令仍存于 Windows 凭据管理器，不随目录迁移。

在 Actions 中手动运行“Windows 构建与发布”，输入已有标签，默认只构建并保存 artifact（14 天）；勾选 `publish` 才发布。上传中断时保留草稿，可重跑失败 job；已经公开的 Release 不覆盖，应发布新标签。同一标签的发布串行执行。

不要移动已用于构建或发布的标签。版本同步只修改 CI 临时工作区，不提交回仓库；桌面版本同步覆盖 `package.json`、`src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml` 和 `src-tauri/Cargo.lock`，Host 版本独立维护。

## 源码范围

提交产品源码、测试夹具、协议、文档、锁文件和构建脚本。以下内容应留在本机：

- `ssh.txt`、`.env`、私钥、凭据、数据库和账号配置。
- `.agentdesk-data/`、`.tmp/`、`test-results/` 及工具缓存。
- `node_modules/`、`dist/`、Rust target 和下载生成的发行资源。
- 包含个人部署目录、账号状态或维护信息的运维记录。

`.gitignore` 防止未跟踪文件被普通 Git 操作加入，但不会移除已经提交的内容。发布前检查候选文件、提交差异和已有 Git 历史。`scripts/check_repository.py` 检查常见私有文件、构建产物和文档链接；CI 中的 Gitleaks 另负责秘密扫描，二者不能互相替代。

```bash
python scripts/check_repository.py
python -m unittest discover -s scripts/tests -v
git status --short
git diff --check
pnpm test
pnpm typecheck
pnpm format:check
pnpm build
pnpm protocol:check
pnpm audit --prod --audit-level=low --registry=https://registry.npmjs.org
uv run --project host python -m pytest host/tests -q
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --locked --manifest-path src-tauri/Cargo.toml
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

真实服务器验收不在普通 CI 中执行，不应向 pull request 工作流注入个人服务器密码或正式账号令牌。其范围见 [验收说明](../scripts/e2e/README.md)。

## 本机 Windows 构建

```bash
python scripts/build/build_host_resources.py
python scripts/build/build_windows_resources.py
pnpm tauri build --ci --target x86_64-pc-windows-msvc --bundles nsis -- --locked
```

构建入口使用锁定依赖和经过摘要核验的官方运行资源。当前产物未签名，Windows 可能显示未知发布者提示；[发布说明模板](../.github/release-notes.md) 如实标明这一点。

如果需要在本机复现某个标签的发行包，在独立干净检出中先运行 `python scripts/build/release.py sync-version --tag v0.1.0`，再执行上述构建命令，最后执行 `python scripts/build/release.py collect-windows --tag v0.1.0`。输出在被 Git 忽略的 `.tmp/release/`，输出目录必须为空，避免混入旧资产。

保留 [LICENSE](../LICENSE) 与 [NOTICE](../NOTICE)。分发安装包前，按实际打包的依赖补齐第三方许可文本和版权清单；现有 NOTICE 仅列主要组件。

## macOS 实验构建

在 Actions 中手动运行 [macOS 实验构建](../.github/workflows/macos.yml)，输入包含本次构建配置的已有标签。工作流针对 Apple Silicon（`aarch64-apple-darwin`），生成 `.app.tar.gz` 和 SHA256，并保存 artifact 14 天。归档保留 app 的执行权限与符号链接，仅包含 Host 资源，不携带 Windows 的 OpenSSH / PowerShell。

macOS 不自动触发，不创建 Release，失败也不影响 Windows 发布。当前没有开发者签名、公证或实机验证，系统可能阻止打开；不宣称可直接交付普通用户。首次实机验证需覆盖窗口、托盘、钥匙串、SSH、文件、终端与 Agent 会话，本机执行通道目前仍仅支持 Windows。Intel Mac 暂不构建。

本机 macOS 构建前，先安装前端与 Rust 依赖、准备 Host 资源，再执行：

```bash
pnpm tauri icon src-tauri/icons/icon.png --output .tmp/macos-icons
pnpm tauri build --ci --no-sign --target aarch64-apple-darwin --bundles app -- --locked
```

图标由现有资源转换，生成文件留在 `.tmp/`；图标清晰度和平台行为待实机验证。

## 发布说明

说明支持的平台、实际验证过的服务器条件、尚未完成的实机项目，以及升级和回退方式。官方账号登录、模型任务、系统缩放或输入法等未执行的检查，不应写为已通过。

Host 当前数据库格式标识为 `3`。新安装直接创建完整结构，同格式数据原样使用，其他格式明确拒绝。首发代码不提供历史数据库迁移或自动备份恢复；不要将不同格式的数据直接交给当前 Host。

安装候选版本在激活前运行 `python -m agentdesk_host --check-database`。手动切换版本在停机前检查目标版本，自动安装恢复在切换链接前检查目标版本；不兼容或不提供检查入口的版本不会被启动。检查失败返回 `E_DATABASE_SCHEMA`，保留数据库与当前版本链接。

不要将历史测试服务器的结果泛化为所有环境均受支持。

## 依赖安全检查

CI 对 npm 生产依赖执行官方 registry 审计，任何级别的告警都会使检查失败。Vue 与 Vue I18n 已更新到修复已知公告的补丁版本；依赖变更需同时更新锁文件并运行界面回归。

Rust 依赖告警需要结合发布目标和调用路径审查，不通过移除 SSH 能力或添加无说明的忽略项来清零：

- [RSA 时序侧信道公告](https://rustsec.org/advisories/RUSTSEC-2023-0071.html)目前没有修复版。当前 `russh` / `ssh-key` 的 RSA 路径用于 SSH 签名和验签，没有 RSA 解密入口；这不表示依赖本身不存在风险。新增加解密能力或更新 SSH 依赖后需重新核验。
- GTK / `glib` 位于非 Windows 桌面依赖链，发布 Linux 桌面版本前需单独处理，不能套用 Windows 验证结果。
- 构建期依赖和停止维护的传递依赖仍需跟踪上游更新；维护状态告警与已确认可利用的运行时漏洞分别记录。
