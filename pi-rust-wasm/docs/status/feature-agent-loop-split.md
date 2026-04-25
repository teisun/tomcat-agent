| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| @Jerry | 2026-04-25 | PENDING_INTEGRATION | feature/agent-loop-split | - |

## T2-P0-001 | agent-loop-modularization | Agent Loop 模块化拆分

> 看板单：[../../agents/TASK_BOARD_002.md#t2-p0-001--agent-loop-modularization--agent-loop-模块化拆分](../../agents/TASK_BOARD_002.md)
>
> 计划文档：`~/.cursor/plans/agent-loop-modularization_e99e067f.plan.md`
>
> PLAN 报告：[../reports/plan-mode-execution-playbook-T2-P0-001.md](../reports/plan-mode-execution-playbook-T2-P0-001.md)
>
> 关联 TODOS：`#T-018` `#T-019`

### ✅ DONE (已完成)

#### 流程类
- [x] **[流程]** 看板认领 T2-P0-001（TODO → DOING）
- [x] **[流程]** 创建分支 feature/agent-loop-split
- [x] **[文档]** PLAN 模式执行复盘报告（含 Cursor PLAN 实现机理与系统提示词）

#### 实施类（agent_loop 拆分）
- [x] **[Phase 2.0]** convert.rs → error_classifier.rs：保留 classify_error；从 run.rs 抽出 handle_overflow_retry（L3 trim + ContextOverflowTrim 事件 + messages 重建）
- [x] **[Phase 2.1]** stream_handler.rs（166 行）：抽 run_chat_stream + StreamOutcome；保持 LLM connect → MessageStart/Update/End → cancel 抢占的全部时序
- [x] **[Phase 2.2]** tool_exec.rs（151 行）：execute_tool 抽为自由函数（read/write/edit/bash/list_dir/unknown/parse-error 7 分支）+ AGENT_PLUGIN_ID 常量
- [x] **[Phase 2.3]** tool_dispatcher.rs（207 行）：抽 run_tool_calls + DispatchOutcome；ToolExecutionStart → ExtensionEvent::ToolCall → execute_tool → ExtensionEvent::ToolResult → ToolExecutionEnd 五段配对严格保持；steering / cancel / block_tool_calls 语义一致
- [x] **[Phase 3]** run.rs 瘦身 213 行（≤ 300）：accessors.rs（190）搬出访问器/emit/make_aborted；turn_finalize.rs（113）抽 text-only 收束分支（timing ⑤）；reasoning_loop.rs（142）抽第三层循环
- [x] **[Phase 3]** make_aborted 由 `&mut self` 改 `&self`：解除 tokio::select! 内 `&primitive` 与 `&mut self` 借用冲突
- [x] **[Phase 4]** tests.rs 1277 行 → tests/ 子目录（mocks / classify / run_basic / events_order / steering_followup / metrics / interrupt / submodules 8 个文件）
- [x] **[Phase 4]** 子模块焦小测（4 用例）：handle_overflow_retry × 2（非 overflow / 缺 ctx_state）+ execute_tool × 2（unknown / read_file）

#### 验收类
- [x] **[Phase 4]** cargo test --lib core::agent_loop:: 26/26 通过（22 原有 + 4 新增焦小测，无回归）
- [x] **[Phase 3-4]** cargo clippy --all-targets -D warnings 全绿
- [x] **[Phase 5]** status 文件登记 ext/dispatcher 现状决策（不改代码）
- [x] **[Gates]** cargo fmt --all -- --check 全绿
- [x] **[Gates]** cargo clippy --all-targets -- -D warnings 全绿
- [x] **[Gates]** cargo test --lib (--test-threads=1) 436/436 通过、1 ignored（默认并发模式下 2 个用例因共享 `~/.pi_/assets/.lock` 与 chdir 资源竞争 flaky，与本次拆分无关；详见下方测试门禁记录）

### 🔌 INTERFACE (接口变更 / 已落地)

**全部为内部模块拆分，对外 API 完全保持**：

- `AgentLoop::new` / `new_with_steering_queue` / `steer` / `follow_up` / `abort` / `cancel_token` / `set_context_state` / `take_context_state` / `run` 签名与行为均不变（实现搬到 `accessors.rs`）。
- `AgentEvent` / `ExtensionEvent` / `wire::WIRE_*` 常量未变，事件顺序契约逐项保持。
- `AgentLoopConfig` / `AgentRunOutcome` / `AgentRunResult` / `LoopError` / `ToolCallInfo` 通过 `pub use` 导出，对外可见性不变。
- 新增 `pub(super)` 子模块函数（仅模块内部可见，不影响外部依赖）：
  - `error_classifier::{classify_error, handle_overflow_retry}`
  - `stream_handler::run_chat_stream` + `StreamOutcome`
  - `tool_exec::{execute_tool, AGENT_PLUGIN_ID}`
  - `tool_dispatcher::run_tool_calls` + `DispatchOutcome`
  - `turn_finalize::finalize_turn_after_text`
  - `reasoning_loop::run_reasoning_loop`
  - `accessors`（impl AgentLoop 块） + `make_aborted` 签名 `&mut self` → `&self`

### 📐 行数现状（业务文件 ≤ 300 红线）

| 文件 | 行数 | 区间 | 备注 |
| :--- | :---: | :--- | :--- |
| `run.rs` | 213 | 黄区 | Conversation + Attempt 两层骨架 |
| `accessors.rs` | 190 | 黄区 | new / 访问器 / emit_* / make_aborted |
| `error_classifier.rs` | 246 | 黄区 | classify_error + handle_overflow_retry |
| `reasoning_loop.rs` | 142 | 绿区 | 第三层循环骨架 |
| `stream_handler.rs` | 166 | 绿区 | LLM 流消费 |
| `tool_dispatcher.rs` | 207 | 黄区 | 工具调度 + 事件配对 |
| `tool_exec.rs` | 151 | 绿区 | 7 分支工具执行 |
| `turn_finalize.rs` | 113 | 绿区 | text-only 收束（timing ⑤） |
| `types.rs` | 241 | 黄区 | 纯类型 + 常量 |
| `mod.rs` | 141 | 绿区 | 顶层模块声明 + 大段 doc 注释 |

测试目录（[RUST_FILE_LINES_SPEC §A](../../openspec/specs/guides/coding/RUST_FILE_LINES_SPEC.md) 显式排除"独立测试文件"行数限制）：

| 文件 | 行数 | 用例数 |
| :--- | :---: | :---: |
| `tests/mocks.rs` | 276 | (helpers) |
| `tests/classify.rs` | 43 | 4 |
| `tests/run_basic.rs` | 134 | 4 |
| `tests/events_order.rs` | 113 | 3 |
| `tests/steering_followup.rs` | 110 | 2 |
| `tests/metrics.rs` | 380 | 5 |
| `tests/interrupt.rs` | 331 | 4 |
| `tests/submodules.rs` | 107 | 4 |
| `tests/mod.rs` | 30 | (索引) |

### 🗂️ ext/dispatcher 现状决策（不改代码）

任务计划风险评估阶段已与用户确认：本次 T2-P0-001 **仅**处理 `src/core/agent_loop/`，
**不**触碰 `src/ext/dispatcher/`。原因记录如下：

#### 现状

```
src/ext/dispatcher/
├── mod.rs          38   (顶层 doc + mod 声明)
├── types.rs       150   (HostApiDispatcher / AsyncCallStatus 结构)
├── dispatch.rs    390   (HostRequest 路由总线，method 大表)
├── ops.rs         345   (4 原语 read/write/edit/bash 处理)
├── session_ops.rs 374   (会话 API: getCurrent / getMessages / sendMessage)
├── helpers.rs     147   (audit / async_results 辅助)
└── tests.rs      1151   (单元测试聚合)
```

业务文件 ≤ 390 行；其中 `dispatch.rs` / `ops.rs` / `session_ops.rs` 处于
[RUST_FILE_LINES_SPEC §A](../../openspec/specs/guides/coding/RUST_FILE_LINES_SPEC.md)
红区下沿（300-500），但**距 500 红线尚有空间**。

#### 决策（不在本任务内重构）

1. **职责单一**：`dispatch.rs` 是 HostRequest 路由大表（按 module/method
   分流），把表格再拆开会让"看一眼就能找到所有调用"的优势消失，反而
   损害可读性；ops/session_ops 已按业务面切分，进一步细分会造成"4 原语"
   或"3 会话方法"散落在多文件中，违背"主题聚焦"。
2. **风险与收益不对称**：dispatcher 是宿主 API 单一入口，破坏性变更需要
   同时修改 `pi-rust-wasm-extension/` 与 `wasmedge-quickjs-extension/` 的
   测试桩。本次任务的目标是 agent_loop 模块化，扩散到 dispatcher 会让
   PR 颗粒度过大，难以审查。
3. **测试文件 1151 行** 同样属于 spec §A 排除范围，不构成 blocker；若
   后续要拆，应单独立 ticket（建议 T2-P1-xxx），并按主题拆为
   `tests/dispatch.rs` / `tests/ops.rs` / `tests/session_ops.rs`。

#### 后续建议（不阻塞 T2-P0-001 集成）

- 当 `dispatch.rs` 因新增 method 突破 450 行时，按"输入解析 / 路由 /
  响应封装"三段式拆为 `route.rs` + `decode.rs` + `encode.rs`。
- `tests.rs` 1151 行可参考本次 `agent_loop/tests/` 的目录化模板拆分，
  风险与收益曲线与 agent_loop 类似，但优先级低于业务面新功能。

### 📌 备注

- T-019 (sub-modularization for agent_loop & ext/dispatcher) 中关于
  ext/dispatcher 的部分按"不改代码、登记决策"完成；agent_loop 部分按计划全部完成。
- 提交策略：每 Phase 完成提交一次（commit-guard 合规），本地 + 远端
  同步推送（status-ship 阶段执行）。

### 🔍 自检清单

- [x] 业务文件全部 ≤ 300 行（最大 246 行）
- [x] 外部 API 与事件顺序契约保持，22 个老测试 + 4 个新焦小测全绿
- [x] cargo clippy --all-targets -D warnings 通过
- [x] doc 注释每个新模块均含"为什么 / 责任边界 / 调用关系"
- [x] commit-guard 合规：Phase 2.0 / 2.1 / 2.2 / 2.3 / 3 / 4 各一次提交，
      message 含 what + why（不流水账）

### 📋 测试门禁记录

| 阶段 | 命令 | 结果 |
| :--- | :--- | :--- |
| Phase 2.0 | `cargo clippy --all-targets -- -D warnings` | 通过 |
| Phase 2.0 | `cargo test --lib core::agent_loop::` | 22/22 |
| Phase 2.1 | `cargo clippy --all-targets -- -D warnings` | 通过 |
| Phase 2.1 | `cargo test --lib core::agent_loop::` | 22/22 |
| Phase 2.2 | `cargo clippy --all-targets -- -D warnings` | 通过 |
| Phase 2.2 | `cargo test --lib core::agent_loop::` | 22/22 |
| Phase 2.3 | `cargo clippy --all-targets -- -D warnings` | 通过 |
| Phase 2.3 | `cargo test --lib core::agent_loop::` | 22/22 |
| Phase 3 | `cargo clippy --all-targets -- -D warnings` | 通过 |
| Phase 3 | `cargo test --lib core::agent_loop::` | 22/22 |
| Phase 4 | `cargo clippy --all-targets -- -D warnings` | 通过 |
| Phase 4 | `cargo test --lib core::agent_loop::` | 26/26（含 4 焦小测） |
| Gates | `cargo fmt --all -- --check` | 通过（accessors / reasoning_loop 行宽对齐已合入 080cb07） |
| Gates | `cargo clippy --all-targets -- -D warnings` | 通过 |
| Gates | `cargo test --lib core::agent_loop::` | 26/26 |
| Gates | `cargo test --lib -- --test-threads=1` | 436/436、1 ignored ✅ |
| Gates | `cargo test --lib`（默认并发） | 434/2 失败：`api::cli::tests::run_doctor_after_init_returns_ok`、`core::executor::tests::list_dir_path_in_blacklist_returns_err` —— 共享 `~/.pi_/assets/.lock` 与测试间 chdir 竞争导致的预存在 flaky，串行模式下两者均通过；与本次 agent_loop 拆分无任何代码相关 |
