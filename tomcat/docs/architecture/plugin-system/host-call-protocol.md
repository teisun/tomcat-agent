# Hostcall 协议、manifest 与能力契约

本文为 [Architecture](../../openspec/specs/Architecture.md) 中「4. 插件系统（统一入口）」的专题页，补充 [`../plugin-system-overview.md`](../plugin-system-overview.md) 的 wire 协议、manifest 字段与 `tools[]` / `functions[]` 契约。

## 文首导读：先把三件事分开

读这份文档时最容易混起来的是三件事：

1. `HostRequest` / `HostResponse`：插件和宿主说话时用的 wire 协议。
2. `plugin.json`：插件静态声明“我大概提供什么能力”的契约面。
3. `pi.registerTool()` / `pi.registerFunction()`：运行时把“当前 VM 里具体哪段 JS 实现可被调用”绑起来。

> 说人话：manifest 更像“身份证 + 申报单”，hostcall 更像“安检窗口的对话格式”，而 `registerTool` / `registerFunction` 才是“把真正的实现塞进这台 VM 的工具箱 / 函数表”。

## Hostcall 入口

当前主链如下：

```text
plugin code
  -> globalThis.pi.*
  -> __pi_host_call(request_json)
  -> invoke_host_func_with()
  -> HostApiDispatcher::dispatch()
  -> HostResponse
```

这条链只承载**真正需要宿主参与**的能力。纯同步 crypto 走原生注入，不经过 dispatcher。

## `HostRequest`

| 字段 | JSON 类型 | 必填 | 说明 | 说人话 |
|------|-----------|------|------|--------|
| `module` | string | 是 | 宿主能力分组，例如 `fs` / `tools` / `llm` / `events` / `session` / `context` / `__async` | 先告诉宿主“你要找哪条业务线”。 |
| `method` | string | 是 | 当前模块下的方法名 | 再告诉宿主“这条线上具体办哪件事”。 |
| `params` | object | 否 | 调用参数 | 具体材料都放这里。 |
| `callId` | string | 否 | 异步 submit / poll 的关联 ID | 这是耗时任务的取件号。 |

## `HostResponse`

| 字段 | JSON 类型 | 必填 | 说明 | 说人话 |
|------|-----------|------|------|--------|
| `ok` | bool | 是 | 调用是否成功 | 成没成先看这个。 |
| `data` | any | 否 | 成功结果；异步 submit 时常见 `{pending:true}` | 真正拿到的结果在这里。 |
| `error` | string | 否 | 失败信息 | 出错原因写在这里。 |
| `callId` | string | 否 | 与异步调用相关的标识 | 方便把结果和之前那次提交对上。 |

## 同步与异步

- **同步**：没有 `callId`，dispatcher 在当前调用链里完成处理并直接返回结果。
- **异步**：带 `callId`，dispatcher 先返回 `pending`，调用方后续通过 `__async.poll` 收结果。

这样做的目的不是追求“协议花样”，而是让：

- 宿主可统一记录超时、取消、审计与错误；
- VM 与宿主之间的执行模型边界保持清晰；
- 工具调用、宿主函数调用、事件循环可以复用同一套生命周期语义。

## 异步 submit / poll 样例

```jsonc
// 异步 submit
{
  "module": "fs",
  "method": "executeBash",
  "params": { "command": "ls" },
  "callId": "req-42"
}

// submit 响应
{
  "ok": true,
  "data": { "pending": true },
  "callId": "req-42"
}

// 轮询
{
  "module": "__async",
  "method": "poll",
  "params": { "callId": "req-42" }
}

// 已就绪响应
{
  "ok": true,
  "data": {
    "ready": true,
    "result": {
      "stdout": "file1.txt\nfile2.txt",
      "stderr": "",
      "exitCode": 0
    }
  }
}
```

## `plugin.json`：静态契约面

当前插件的静态事实源是 `plugin.json`。常用字段如下：

| 字段 | 作用 | 说人话 |
|------|------|--------|
| `id` / `name` / `version` | 标识插件 | 先把插件是谁说清楚。 |
| `main` | JS / TS 入口 | 告诉宿主“真正的实现从哪个文件开始跑”。 |
| `requiredPermissions` | 插件声明的权限需求 | 像一张权限申报单，本期主要用于记录与后续权限系统衔接。 |
| `tools[]` | 给 LLM 的静态工具契约 | 这是“要摆上模型工具架的说明书”。 |
| `functions[]` | 给宿主的静态扩展点契约 | 这是“只给系统内部看的能力申报单”。 |
| `events[]` | 插件声明会消费的事件名 | 告诉宿主“我关心哪些事件”。 |
| `activation` | 运行时激活策略，例如 `lazy` / `session` | 决定要不要在会话进入时就让活体 VM 在场。 |

## `tools[]`、`functions[]`、skill 的边界

| 维度 | `tools[]` | `functions[]` | skill | 说人话 |
|------|-----------|---------------|-------|--------|
| 消费方 | LLM | 宿主子系统 | 模型按需装载正文 | 三者受众不同，不能混账。 |
| 注册面 | `ToolRegistry` | `FunctionRegistry` | `load_skill(name)` | 工具是可执行能力；函数是宿主扩展点；skill 是提示资产。 |
| 典型用途 | tool calling | `web_search.backend` 这类宿主扩展点 | 让模型学一套操作套路 | 一个给模型用，一个给系统用，一个给提示词用。 |
| 是否暴露给 LLM | 是 | 否 | 元数据常驻、正文按需读 | `functions[]` 永远不应该污染 LLM 工具表。 |

需要特别强调两点：

1. manifest 负责声明“**当前插件能暴露什么能力**”；
2. `pi.registerTool()` / `pi.registerFunction()` 负责把“**当前 VM 里实际该调哪段 JS 实现**”绑定起来。

也就是说：**静态可见性** 与 **运行时实现绑定** 是两层不同的事实。

## manifest 为什么只报“最小契约”

`functions[]` 不应该把插件内部实现细节全部上浮给宿主。对 `web_search.backend` 这类点，本期更推荐只声明类似：

```json
{
  "point": "web_search.backend",
  "function": "webSearchBackend"
}
```

这意味着：

- 宿主只知道“当前 scope 下，这个插件愿意提供 `web_search.backend` 这类能力”；
- 真正支持哪些 vendor、默认顺序、auto/fallback 逻辑仍留在插件代码里；
- `FunctionRegistry` 看到的是一个**宿主可消费的稳定契约**，不是插件内部配置中心。

> 说人话：宿主只需要知道“你会不会做这件事，以及该叫你哪个函数名”；至于插件内部怎么排后端、怎么兜底，不该变成宿主要维护的表。

## 同步原生 crypto（不经 dispatcher）

| 全局函数 | 入参 | 出参 | 说明 | 说人话 |
|----------|------|------|------|--------|
| `__pi_crypto_hash_native` | `(algo, data, encoding)` | string | sha256/384/512/sha1/md5 | 纯算的活别绕宿主分发器，直接走原生速度。 |
| `__pi_crypto_random_uuid_native` | `()` | string | v4 UUID | 发一个真正随机的 UUID。 |
| `__pi_crypto_random_bytes_native` | `(size)` | hex string | 随机字节 | 要随机数时直接给。 |

## 边界约束

- `node:fs`、`node:child_process`、`node:os` 不直接进入插件运行时。
- 文件、命令、LLM、会话、事件总线等敏感能力必须走 hostcall。
- `functions[]` 不进入 LLM 的工具清单。
- 运行时已经不是 Wasm guest，不再讨论线性内存读写协议。

## 单一事实源

- 协议 DTO：`src/ext/host_binding.rs`
- 分发入口：`src/ext/dispatcher/`
- JS bridge：`assets/js/pi_bridge.js`
- 类型提示：`assets/types/tomcat-plugin.d.ts`

## 与其他文档的关系

- JS bridge 与宿主 API 分层：[`js-bridge-and-host-api.md`](./js-bridge-and-host-api.md)
- 发现、scope 与激活时机：[`plugin-source-scan-register-load.md`](./plugin-source-scan-register-load.md)
- 运行时实例与隔离：[`runtime-and-sandbox.md`](./runtime-and-sandbox.md)
