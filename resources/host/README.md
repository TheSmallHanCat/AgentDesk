# Host 发行资源

执行 `python scripts/build_host_resources.py` 生成 Host wheel 与 SHA256 manifest。生成文件不提交。

桌面安装器上传当前构建的 wheel；uv、Codex 与 bwrap 从 manifest 固定的官方发行 URL 下载并核对 SHA256。Claude SDK 版本固定为 0.2.174，CLI 使用该 SDK 随附二进制。不会改用户 PATH、shell 文件或既有 CLI 配置。发布打包前必须准备资源，缺少 wheel 时安装计划明确报错。
