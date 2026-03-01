# Pi 生态兼容性对齐检查报告

基于 `design.md`、`Architecture.md` 与 **pi_agent_rust**、**pi-mono** 的对照，影响兼容性的技术设计对齐情况如下。

---

## 1. 事件系统

### 1.1 当前 design.md / Architecture.md 状态

- **design.md [CODE_BLOCK_P1_003]**：单一 `AgentEvent` 枚举，变体为 PascalCase（SessionCreate、BeforeUserMessage、BeforeToolCall、AfterToolCall、ToolRegister、ToolUnregister 等），无 payload 结构。
- **Architecture.md**：同样为单一 `AgentEvent` 枚举，且「事件执行机制」「安全设计核心原则」两段未使用规范 Markdown 列表。

### 1.2 pi_agent_rust 实际设计

- **AgentEvent**（`src/agent.rs`）：用于**流式/UI 订阅**，序列化 `type` 为 snake_case（`agent_start`、`turn_start`、`message_start`、`tool_execution_start`、`tool_execution_end`、`auto_compaction_start`、`auto_retry_start`、`extension_error` 等），payload 字段为 **camelCase**（如 `sessionId`、`toolResults`）。
- **ExtensionEvent**（`src/extension_events.rs`）：用于**扩展钩子**，事件名为 snake_case 字符串：`startup`、`agent_start`、`agent_end`、`turn_start`、`turn_end`、`tool_call`、`tool_result`、`session_before_switch`、`session_before_fork`、`input`；payload 使用 camelCase。

### 1.3 pi-mono 实际设计

- 扩展通过 `pi.on("session_start", ...)`、`pi.on("tool_call", ...)` 等**字符串事件名**监听。
- 事件名与 pi_agent_rust ExtensionEvent 一致：`session_start`、`session_before_switch`、`session_switch`、`session_before_fork`、`session_fork`、`session_before_compact`、`session_compact`、`session_shutdown`、`session_before_tree`、`session_tree`、`context`、`before_agent_start`、`agent_start`、`agent_end`、`turn_start`、`turn_end`、`message_start`/`message_update`/`message_end`、`tool_execution_start`/`update`/`end`、`model_select`、`tool_call`、`tool_result`、`user_bash`、`input`、`resources_discover`。

### 1.4 对齐结论（事件）

| 项目 | 设计/架构文档现状 | 与 pi 对齐要求 |
|------|-------------------|----------------|
| 事件分类 | 单一 AgentEvent | 拆成 **AgentEvent**（流式/UI）与 **ExtensionEvent**（扩展钩子） |
| 事件名 | 仅 PascalCase 变体名 | 序列化 `type` 统一 **snake_case**（如 `tool_call`、`session_before_switch`） |
| 扩展监听 API | EventBus 使用 `on(event: AgentEvent, ...)` | 扩展侧应为 **字符串事件名** `on("tool_call", ...)`，与 pi-mono/pi_agent_rust 一致 |
| 流式事件 | 未区分 | 需有 **AgentEvent**（含 agent_start、turn_start、message_start、tool_execution_*、auto_compaction_*、auto_retry_*、extension_error）及 payload 的 camelCase 约定 |
| 扩展事件 | 无 startup、input 等 | **ExtensionEvent** 需包含 startup、input、tool_call、tool_result、session_before_switch、session_before_fork 等，与 pi 一致 |

---

## 2. 宿主 API（ExtensionAPI）

### 2.1 design.md 表中所列

- 4 原语：readFile / writeFile / editFile / executeBash  
- LLM：createChatCompletion / createChatCompletionStream  
- 工具：registerTool / unregisterTool / getToolList / callTool  
- 事件：on / once / off / emit  
- 会话：getCurrentSession / getMessages / sendMessage / updateSessionConfig  
- 配置：getPluginConfig / setPluginConfig / getGlobalConfig  

### 2.2 pi_agent_rust 实际暴露

- 4 原语：以 **hostcall** 形式暴露为 **tool.read / tool.write / tool.edit / tool.bash**（JS 侧通过工具调用链使用，不是独立的 readFile()）。
- 会话：session.get_state、session.get_messages、session.get_name、session.set_name、session.get_model、session.set_model 等。
- 事件：events.emit、events.append_entry、events.register_command 等。
- 文档（ext-compat.md）中还有 pi.tool(name, input)、pi.session.*、pi.events(op, payload)。

### 2.3 pi-mono 实际暴露

- **sendMessage**、**appendEntry**、**registerTool**、**on(eventName, handler)**、**emit**；会话与配置通过 context 上的 sessionManager、配置接口等提供。
- 4 原语在 pi-mono 中为**内置工具** read / write / edit / bash，由 LLM 通过工具调用使用，扩展不直接调用 readFile()。

### 2.4 对齐结论（API）

| 项目 | 建议 |
|------|------|
| 4 原语 | 与 pi 一致：以**内置工具** read/write/edit/bash 暴露给 LLM；若扩展需直接调宿主能力，可提供与 pi_agent_rust 一致的 tool 调用通道（如 tool.read(path) 等），命名需与 pi 约定一致，而非仅 readFile。 |
| 事件 API | 扩展侧为 **on(event_name: string, ...)**、**emit(event_name, payload)**，事件名为 snake_case 字符串，与 pi-mono 文档一致。 |
| 会话/配置 | getCurrentSession、getMessages、sendMessage 等命名与 pi-mono 概念对齐即可；getPluginConfig/setPluginConfig 与 pi 的“扩展自身配置”对齐。 |

---

## 3. 工具定义（Tool / ToolDefinition）

### 3.1 design.md [CODE_BLOCK_P1_007]

- `Tool` 含：name、description、parameters（JSON）、**handler: String**、plugin_id、is_enabled、created_at。

### 3.2 pi-mono ToolDefinition

- **name**、**label**、**description**、**parameters**（TypeBox/JSON Schema）、**execute**(toolCallId, params, signal, onUpdate, ctx) → Promise<AgentToolResult>；可选 renderCall、renderResult。

### 3.3 对齐结论（工具）

- **handler** 不应为 String：与 pi-mono 一致应为可执行体（在 Rust 侧为回调/函数指针，在 JS 侧为 execute 函数）。
- 建议增加 **label**；**parameters** 明确为 JSON Schema；返回值形态与 **AgentToolResult**（content、details 等）对齐。

---

## 4. 设计文档与架构文档的同步

### 4.1 建议

1. **事件**：以 **Architecture.md** 为事件设计的唯一权威：定义 AgentEvent（流式）+ ExtensionEvent（扩展钩子），事件名 snake_case，payload camelCase；**design.md** 中删除与 Architecture 矛盾的单一 AgentEvent 枚举，改为引用「见 Architecture.md 事件系统设计」，并说明扩展钩子使用**字符串事件名**（on/emit）。
2. **EventBus Trait**：若 Rust 内部仍用枚举分发，扩展对外仍应暴露 **字符串事件名**（如 `on("tool_call", handler)`），与 pi-mono/pi_agent_rust 一致；design.md 的 EventBus 可注明「内部映射到 ExtensionEvent，对外 API 为字符串事件名」。
3. **API 表**：在 design.md 中补充说明：4 原语以内置工具 read/write/edit/bash 提供，与 pi-mono 一致；事件名与 pi 文档一致（snake_case 字符串）。
4. **Tool 结构**：design.md 中 Tool 定义与 pi-mono ToolDefinition 对齐（name、label、description、parameters、execute 语义），去掉 handler: String。

---

## 5. 总结

| 维度 | 是否对齐 | 说明 |
|------|----------|------|
| 事件分类与命名 | 否 | 需拆分为 AgentEvent + ExtensionEvent，事件名 snake_case，扩展侧使用字符串事件名 |
| 事件 payload | 部分 | Architecture 需明确 payload 的 camelCase 与 pi_agent_rust 一致 |
| 4 原语暴露方式 | 需澄清 | 应以内置工具 read/write/edit/bash 为主，与 pi-mono 一致；若提供直接 API，命名与 pi_agent_rust 一致 |
| 事件 API（on/emit） | 否 | 应为 on(event_name: string, ...)，与 pi-mono 一致 |
| 工具定义 | 否 | Tool 应含 label、parameters 为 JSON Schema、execute 语义，无 handler: String |
| 会话/配置/LLM API | 基本 | 命名与 pi 概念一致即可，设计文档已表格式列出 |

完成上述修改后，**影响兼容 pi 生态的技术设计**可与 pi_agent_rust、pi-mono 对齐。
