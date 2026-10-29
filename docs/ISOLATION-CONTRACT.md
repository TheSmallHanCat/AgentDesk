# 服务器隔离工作区

隔离工作区是可选运行方式。桌面与 Host 仍通过 SSH 通信，Host 保留项目索引、事件和审批；官方 Agent 由独立容器内的 Worker 启动。当前实现优先支持 Claude。普通模式中的 Claude、Codex 和本机项目继续使用原有路径。

本机代码与协议测试不能代替 Linux 容器验收。服务器必须通过实际边界探测，才能启动隔离会话；失败时不退回普通模式，也不提升容器或 Codex 权限。

## 项目与会话归属

- `Project.runtimeMode` 为 `direct` 或 `isolated`，省略时为 `direct`。已有项目不能改变运行方式；需要不同运行方式时新建项目索引。
- Host 为每个隔离项目生成固定 `runtimeId`，会话创建时记录相同 ID。恢复、发送、登录与能力检测均从持久归属解析环境，不接受调用方指定容器或挂载路径。
- 隔离模式仅接受服务器目录。拒绝本机项目、覆盖 Host 数据的目录、SSH 配置目录、系统敏感目录及指向外部 Git 元数据的 worktree。
- 旧项目与旧会话的运行方式保持 `direct`，不搬动文件，不复制令牌或原生会话。Host 数据库增加字段并提升 schema 版本；旧版 Host 不能直接读取升级后的数据库。
- 握手提供 `features.isolatedRuntime=1`。桌面遇到不支持此能力的 Host，或 Host 未返回隔离归属时，拒绝隔离操作。

## 容器边界

运行端使用 rootless Podman 和 cgroup v2。镜像配置只接受完整仓库摘要或不可变的 `sha256:` 镜像 ID；启动使用 `--pull=never`，不自动拉取镜像。

每个运行环境只挂载当前项目到 `/workspace`、专用持久目录到 `/home/agent`。容器根文件系统只读，使用独立 PID、网络、IPC、UTS、用户与 cgroup 命名空间，移除 capabilities 并启用 no-new-privileges。默认限制为 2 GiB 内存、2 CPU、256 个进程。最多运行 8 个工作区 Worker；达到上限时回收无任务、登录或待处理请求的空闲 Worker。全部忙碌时拒绝新增 Worker，现有任务继续运行。

启动器先创建并检查容器的所有权、镜像、挂载、用户、命名空间与资源配置，随后启动 Worker。Worker 再检查实际命名空间、根文件系统、权限、seccomp、资源限制和工作目录可写性。OCI 配置或运行结果不符即拒绝执行。

容器没有 Host 控制 socket、数据库、设备密钥、宿主 SSH agent 或容器管理 socket。容器镜像只提供自己的工具链；不会把宿主机整个目录或工具链自动挂载进去。隔离模式首期不提供独立 Shell 和原生 Agent 终端，也不启用本机反向通道。项目命令通过 Agent 的原生工具在容器内执行。

显式关闭 Podman 的宿主环境与代理变量自动继承。运行环境沿用镜像配置，不从桌面传递时区、语言或代理；需要额外工具和环境配置时由管理员维护镜像。

Host 用户与服务器管理员仍是可信控制方。容器不能防御宿主管理员、内核漏洞、显式上传的敏感内容或官方登录浏览器提供的信息，也不构成账号访问保证。

## 登录与运行协议

每个工作区使用自己的官方登录与原生会话目录，第一次使用需要官方授权。前端能力缓存按运行环境区分，不把普通环境的已登录状态应用到隔离环境。授权链接和代码仍是临时通知，不进入事件数据库。

Worker 使用有帧上限的 stdio 请求和响应，复用现有 Agent 适配器。跨边界只传递当前会话的 Agent、模型、审批模式、原生会话 ID 和用户输入；容器内工作目录固定为 `/workspace`，不传桌面设备名、本机路径或 Host 连接参数。

审批与原生会话 ID 回传绑定原 runId，由 Host 处理并确认。事件继续由 Host 分配序号和持久化。追加输入的实际送达状态在运行结束时回传；Worker 异常退出导致无法确认时显示“状态未知”，不自动重发。

`agent.status`、`env.probe`、`login.start`、`agent.logout` 支持可选 `projectId`，由 Host 解析其固定环境。`runtime.status` 只检查运行工具与镜像，不启动宿主 Agent。握手与 Host 启动不再主动启动普通环境中的 CLI；检测由对应作用域的请求触发。

桌面断开只取消其登录过程，服务器上的活动运行可以继续。停止结束当前 Agent 任务；Worker 空闲回收或 Host 退出时清理所属容器，保留持久登录和原生会话目录。清理失败时保留所有权记录并阻止新容器启动。Host 重启后将旧运行标记为中断，并在下一次隔离启动前清理带有本 Host 所有权标签的遗留容器。不会删除持久目录或其他用户容器。

清理失败后不在普通请求中自动重试。确认 Podman 恢复可用并结束其他任务后，重启 Host 以重试所属容器清理。

## 准备与启用

先准备 Host wheel 与锁定依赖：

```bash
python scripts/build/build_host_resources.py
```

`scripts/build/prepare_isolation_image.py` 要求 `--base-image` 为经核验的官方 Python 3.12 Debian 基础镜像完整摘要，`--output` 为不存在的构建目录。默认只生成上下文；显式增加 `--build` 才调用本机 rootless Podman。基础镜像需已存在本机，构建不自动拉取基础镜像；安装锁定 Python 依赖和系统构建组件需要网络。

构建工具检查 wheel 与当前源码一致，输出构建后镜像的不可变 ID。将输出的 `isolation.image` 配置合并到服务器 `~/.agentdesk/config.json`，再重启 Host。该步骤由用户或管理员执行，不由桌面静默更改系统设置。

创建项目时选择“服务器隔离工作区”。运行工具和镜像检查通过后可以保存索引；进入工作台时执行实际容器边界探测，随后在该工作区完成官方登录并创建会话。

## 验证与回退

本地可运行 `uv run --project host python -m pytest host/tests/test_isolation.py -q`，以及 `scripts/browser/smoke_isolation.py`。这些检查使用临时数据、真实 Python 子进程和 Fake Adapter，不调用模型。

Linux 实机验收使用 `python scripts/e2e/smoke_isolation_runtime.py --image IMAGE_ID`，其中 `IMAGE_ID` 为已准备镜像的实际不可变 ID。脚本只创建所属临时目录和容器，验证边界及 Host 标记不可见，再清理临时资源；不运行 Agent 登录或模型请求。正式使用前还需要用户授权后的 Claude 登录、真实任务、重连、审批与停止验收。

停用时先停止隔离任务，再归档对应项目；持久目录保留以供恢复。不要将隔离会话改为普通环境恢复。二进制降级需要部署前的数据库备份，不能让旧 Host 强行读取新 schema；本实现不自动执行备份恢复、用户数据迁移或删除。

实现依据：[Podman rootless 前置条件](https://docs.podman.io/en/latest/markdown/podman.1.html#rootless-mode)、[容器运行选项](https://docs.podman.io/en/latest/markdown/podman-run.1.html)。具体 Linux 环境的可用性以运行探测和实机验收为准。
