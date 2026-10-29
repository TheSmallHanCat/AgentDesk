# 开发脚本

所有命令示例从仓库根目录执行。脚本根据自身位置定位仓库，不依赖将服务器地址或凭据写入源码。

| 目录 / 入口           | 用途                                                       | 外部影响                                             |
| --------------------- | ---------------------------------------------------------- | ---------------------------------------------------- |
| `build/`              | 准备 Host、Windows 执行端和可选容器镜像的构建资源          | 部分命令下载固定版本资产；容器构建需要显式 `--build` |
| `browser/`            | 浏览器界面和交互回归，使用 `tests/fixtures/` 中的 IPC 夹具 | 不连接真实服务器，不调用模型                         |
| `e2e/`                | 真实桌面、SSH、Host、执行通道和 Linux 容器验收             | 逐项阅读说明；部分命令会安装、改配置或创建验收数据   |
| `check_repository.py` | 检查公开候选文件和文档链接                                 | 只读，不连接网络                                     |
| `tests/`              | 发布标签、版本同步和打包边界单元测试                       | 临时目录中运行，不连接网络                           |

## 常用入口

```bash
python scripts/build/build_host_resources.py
python scripts/build/build_windows_resources.py --check
python scripts/check_repository.py
python scripts/build/check_tauri_versions.py
python -m unittest discover -s scripts/tests -v
```

`build_windows_resources.py --check` 只核验已有资源；首次准备时去掉 `--check`。

`build/check_tauri_versions.py` 只读检查桌面 JavaScript / Rust 的 Tauri 核心及插件版本，发布前运行，不连接网络。

`build/release.py` 用于 CI 中解析标签、同步桌面版本并收集 Windows 安装包、便携包和 SHA256。标签发布与手动构建的完整入口见 [发布说明](../docs/RELEASING.md)。版本同步会修改工作区版本文件，应在 CI 或独立检出中执行；收集产物要求输出目录为空。

启动 `pnpm dev --host 127.0.0.1` 后，在另一个终端运行：

```bash
python scripts/browser/smoke_workflow.py --url http://127.0.0.1:1420
python scripts/browser/smoke_login.py --url http://127.0.0.1:1420
python scripts/browser/smoke_isolation.py --url http://127.0.0.1:1420
```

浏览器脚本需要 Python Playwright 与 Chromium，可通过 `python -m pip install playwright` 和 `python -m playwright install chromium` 准备。本地单元测试不需要浏览器。

真实环境入口和权限范围见 [验收说明](e2e/README.md)。`e2e/requirements.txt` 提供 SSH 验收脚本使用的额外依赖；不要将整个目录作为一个无人值守的测试套件执行。

早期技术实验仍放在 [spikes/](../spikes/README.md)，不属于产品构建入口。新增脚本按上述三类归组，保持辅助模块与调用方的导入关系。
