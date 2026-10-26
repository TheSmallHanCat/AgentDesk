# Host 协议

`hello.schema.json` 定义握手和目标；`m2.schema.json` 定义 M2 会话、运行、事件、审批、补取和 blob 类型。`x-methods` 给出首版方法的参数与结果引用；M3/P1 方法仅声明协议并返回 `E_CAPABILITY`。

```bash
python protocol/generate.py
python protocol/generate.py --check
```

生成器仅依赖 Python 标准库，合并当前目录下所有 Schema 的 `$defs`，拒绝重复类型名与未知类型。支持封闭对象、字典、数组、基础类型、const/enum、本地引用、anyOf/oneOf、null 和显式任意 JSON。Rust 的异构联合使用 `serde_json::Value`，Python 的任意 JSON 使用 `Any`；Schema 保留完整运行时约束。

三端输出：`src/protocol/generated.ts`、`src-tauri/src/host/protocol.rs`、`host/agentdesk_host/protocol.py`。生成类型不代替运行时校验：Host 明确检查握手版本、目标归属、权限、参数和 4 MiB 帧上限，测试以 JSON Schema 验证线格式。

事件 seq 为 Host 全局单调序号，允许因未落库的 delta 形成间隙。`sync.since` 分页期间水位只前进到已返回事件，最后一页才返回当前水位和进行中快照。`event.blob` 返回原始完整 payload JSON 的 UTF-8 文本，offset/nextOffset/total 均为字节数；调用方解析合并后的 JSON 恢复完整内容。

审批同时以通知与带 id 的 `approval.request` 服务端请求推送，params 为相同的 HostEvent。客户端可用 `result: {decision}` 回复，也可继续调用 `approval.respond`。服务端只接受发给该连接的请求 id，决定设备始终来自握手；已生效决定由同一审批事务保证。
