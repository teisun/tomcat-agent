# 项目 TODO 汇总

> 生成时间：2026-04-03
> 来源：code review 跟踪项（`docs/status/develop.md:38`）+ 全项目 `TODO` / `FIXME` / `DEPRECATED` 扫描
> 排除：`assets/modules/` 下上游三方 JS 模块中的 TODO

---

## 1. 高优先级跟踪项（code review）

| # | 标题 | 位置 | 描述 |
|---|------|------|------|
| H-1 | chat.rs CascadeResult 被丢弃 | `src/api/chat.rs:304-312` | 预检 cascade 调用 `run_compaction_cascade_v2` 返回值被丢弃，`block_tool_calls` 未传递给后续构造的 `AgentLoop`（`AgentLoop` 内部 overflow 重试和每轮 cascade 路径正常，仅预检缺失） |
| H-2 | circuit_breaker_skips_layer2 断言偏弱 | `src/core/compaction.rs:1037-1042` | 测试仅断言 setup（`failures >= 3`），未调用 cascade、未验证 Layer 2 被跳过或 Layer 3 被触发 |
| H-3 | PTL retry 缺专项测试 | `src/core/compaction.rs:489-523` | `retry_with_half_range` 为模块私有函数，无直接单元测试覆盖"首次 summarization overflow → 半区间重试 → 成功/耗尽"路径 |
| H-4 | `let _ = work_dir_str` 死变量残留 | `src/api/chat.rs:303,313` | `work_dir_str` 绑定后仅被 `let _` 消费，应移除或实际使用 |

---

## 2. ContextMetricsUpdate 未接线（token 水位 UI 缺失）

**事件定义**：`src/infra/events.rs:202-212`

```rust
ContextMetricsUpdate {
    input_tokens_used: usize,
    context_utilization_ratio: f64,
    compaction_count: u32,
    compaction_tokens_freed: usize,
    total_tool_result_bytes_persisted: usize,
}
```

**现状**：全 `src/` 无 `emit_event(AgentEvent::ContextMetricsUpdate { .. })` 调用。CLI 终端无法显示 token 预算水位；用户仅在工具被 block 时间接感知（`src/core/agent_loop.rs:779-794`）。

**建议接入点**：`agent_loop.rs` 每轮 cascade 后（约 877-901 行）emit `ContextMetricsUpdate`；CLI 侧订阅该事件后在提示符旁显示 ratio。

---

## 3. 第一方 Rust TODO（src/）

| 位置 | 内容 |
|------|------|
| `src/core/agent_loop.rs:320` | `max_tool_rounds` 待 tool-loop-detection 方案替代 |
| `src/core/llm/openai.rs:49` | `stream_timeout_sec` 待接入 `tokio::time::timeout` 实现流式超时 |
| `src/ext/dispatcher.rs:940` | 事件注册占位回调，待长生命周期 VM 就绪后注入真实实现 |

---

## 4. WasmEdge / JS API 测试迁移 TODO

所有标注 `// TODO: migrate to long-lived VM (see plan §3)` 的测试：

| 文件 | 行号 |
|------|------|
| `tests/wasmedge_e2e_tests.rs` | 153, 211, 317, 363, 429, 485, 547, 605, 675 |
| `tests/js_api_alignment_tests.rs` | 67, 124 |

---

## 5. Deprecated 标记

| 位置 | 说明 |
|------|------|
| `src/core/agent_loop.rs:299-300` | `compact_messages` 已弃用，由 token-aware ContextState + 四层防护替代（TASK-17） |
| `src/ext/instance_wasmedge.rs:221-231` | `dispatch_event` 已弃用，由 `PluginManager::dispatch_session_event` + 长生命周期 VM actor 替代 |

---

## 6. 规格 / 文档级 TODO（低优先级）

| 位置 | 内容 |
|------|------|
| `openspec/specs/Constitution.md:16` | 敏感数据加密 |
| `openspec/specs/architecture/context-management.md:744` | `max_tool_rounds` 硬限制待替代 |
| `openspec/specs/User_Stories.md:14,75,174` | 加密存储相关 |
| `openspec/specs/Product_Brief.md:34` | 产品级 TODO |
| `openspec/changes/001-mvp/design.md:52,303` | MVP 设计伪代码中的加密 TODO |
| `docs/user-guide.md:587` | 审计日志加密为后续 TODO |
