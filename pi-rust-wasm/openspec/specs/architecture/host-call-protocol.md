# Hostcall JSON 协议

本文为 [Architecture](../Architecture.md) 中「3. 宿主API层」的协议子文档，定义 Wasm/JS 与宿主之间通过 `__pi_host_call` 交换的 **请求/响应 JSON 格式**。实现以 [host_binding.rs](../../../src/ext/host_binding.rs) 与 [dispatcher.rs](../../../src/ext/dispatcher.rs) 为准，本文档为权威协议说明。

---

## 1. 传输约定

- **入口**：宿主向 Wasm 注册 `env.__pi_host_call`（签名：线性内存 ptr + len，或由运行时封装为「请求 JSON 字符串 → 响应 JSON 字符串」）。
- **方向**：Guest（JS/Wasm）构造请求 JSON 字符串，调用 `__pi_host_call(requestJson)`；Host 解析后按 `module`/`method` 分发，返回响应 JSON 字符串。
- **编码**：UTF-8 JSON，**字段名统一 camelCase**，与 Rust 侧 `#[serde(rename_all = "camelCase")]` 一致。

---

## 2. 请求体：HostRequest

Guest 发给宿主的一笔调用，对应单次 `__pi_host_call` 的入参（整段为 JSON 字符串）。

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `module` | string | 是 | 模块标识，如 `"fs"`、`"primitive"`、`"llm"`、`"agent"`、`"tools"`、`"events"`、`"session"`。 |
| `method` | string | 是 | 方法名，如 `"readFile"`、`"writeFile"`、`"createChatCompletion"`。 |
| `params` | object | 是 | 方法参数，见下节「按 module/method 的 params 约定」。可为 `{}`。 |
| `callId` | string | 否 | 调用 ID，用于异步回传关联；同步调用可省略。 |

示例：

```json
{
  "module": "fs",
  "method": "readFile",
  "params": { "path": "/tmp/foo.txt" }
}
```

---

## 3. 响应体：HostResponse

宿主返回给 Guest 的 JSON 字符串，对应单次 `__pi_host_call` 的返回值。

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `ok` | boolean | 是 | 是否成功。 |
| `data` | object/any | 否 | 成功时的结果数据；失败时通常不填。 |
| `error` | string | 否 | 失败时的错误信息。 |
| `callId` | string | 否 | 与请求中的 `callId` 对应，异步回调时必填。 |

示例（成功）：

```json
{ "ok": true, "data": { "content": "file content here" } }
```

示例（失败）：

```json
{ "ok": false, "error": "readFile: missing path" }
```

---

## 4. 按 module / method 的 params 约定

以下与当前 [dispatcher.rs](../../../src/ext/dispatcher.rs) 实现一致；新增 API 时须同步更新本文档。

### 4.1 fs / primitive（4 原语）

- **readFile**  
  - `params.path` (string, 必填)：文件路径。  
  - 响应成功时 `data.content` 为文件内容字符串。

- **writeFile**  
  - `params.path` (string, 必填)：文件路径。  
  - `params.content` (string, 可选)：写入内容，默认 `""`。  
  - `params.overwrite` (boolean, 可选)：是否覆盖已有文件，默认 `false`。

- **editFile**  
  - `params.path` (string, 必填)：文件路径。  
  - `params.edits` (array, 可选)：编辑操作列表，每项含 `type`、`old`、`new` 等，与 `EditOperation` 结构一致；默认 `[]`。

- **executeBash**  
  - `params.command` (string, 必填)：要执行的 shell 命令。  
  - `params.cwd` (string, 可选)：工作目录。

### 4.2 agent

- **log** / **debug**：日志类接口，`params` 由实现定义。

### 4.3 llm

- **createChatCompletion** / **createChatCompletionStream**：参数与 pi-mono LLM 接口对齐，见实现与 pi-mono 文档。

### 4.4 tools / events / session

- **tools**：`registerTool`、`unregisterTool`、`getToolList`、`callTool` 等，params 见 dispatcher 与 pi-mono 约定。  
- **events**：`on`、`off`、`once`、`emit`，params 为事件名与 payload。  
- **session**：`getCurrentSession`、`getMessages`、`sendMessage` 等，params 见实现。

---

## 5. 与实现文件的对应关系

- **请求/响应结构**：[src/ext/host_binding.rs](../../../src/ext/host_binding.rs) 中 `HostRequest`、`HostResponse`。  
- **分发与 params 解析**：[src/ext/dispatcher.rs](../../../src/ext/dispatcher.rs) 中按 `(module, method)` 路由及各 `do_*` 方法。  
- **Wasm 侧调用**：`env.__pi_host_call` 的注册与线性内存读写见 [wasmedge-runtime-layer](wasmedge-runtime-layer.md) 与 [instance_wasmedge.rs](../../../src/ext/instance_wasmedge.rs)。
- **执行时注入**：每次执行 `run_script`/`run_script_file` 时，当次 Vm 在运行 quickjs 的 `_start` 前已挂载 env 模块及 `env.__pi_host_call`。**Guest 侧**（wasmedge_quickjs.wasm）须从 env 导入该函数并暴露给 JS（预编译的 wasm 若未暴露则 JS 无法调用，需定制 wasm 或胶水层）；JS 调用约定见本文档第 5 节本段下方。

JS 侧若运行时暴露为「字符串进、字符串出」，则直接使用：`const response = __pi_host_call(JSON.stringify(request)); const res = JSON.parse(response);`。
