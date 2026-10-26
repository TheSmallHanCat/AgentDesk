# 协议样例

`hello.schema.json` 定义 NDJSON JSON-RPC 2.0 握手及 Target 样例。此阶段只有 Schema、类型和测试，不包含传输服务或 Host 方法实现。

```bash
python protocol/generate.py
python protocol/generate.py --check
```

生成器使用 Python 标准库，对当前对象、字典、数组、基础类型、枚举和本地引用提供确定性输出。无法识别的类型直接报错；扩展 Schema 时需同时扩展生成器及测试，不能静默退化为任意类型。

输出为 `src/protocol/generated.ts`、`src-tauri/src/host/protocol.rs`、`host/agentdesk_host/protocol.py`。Rust/Python 结构体用于类型约束，不完整执行 JSON Schema 的 const、enum、minimum 等运行时校验；实际 hello 服务在 M2 必须显式校验协议版本和帧大小。Python 测试对请求/响应样例进行完整 JSON Schema 校验。
