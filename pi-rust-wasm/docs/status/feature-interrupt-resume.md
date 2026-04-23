| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| @Spike | 2026-04-22 | PENDING_INTEGRATION | feature/interrupt-resume | - |

## T2-P0-007 | interrupt-resume-transcript | 中断 / 恢复 + transcript 完整性

> 看板单：[../../agents/TASK_BOARD_002.md#t2-p0-007--interrupt-resume-transcript--中断恢复--transcript-完整性](../../agents/TASK_BOARD_002.md)
>
> 计划文档：`~/.cursor/plans/interruptible_agent_loop_c77e96ab.plan.md`
>
> 架构文档：[../../openspec/specs/architecture/interrupt-and-cancellation.md](../../openspec/specs/architecture/interrupt-and-cancellation.md)
>
> 关联 TODOS：`#T-003` `#T-004` `#T-007`（最小版） `#T-017`

### ✅ DONE (已完成，待 Nibbles 集成复核)

#### 流程类
- [x] **[流程]** 看板认领 T2-P0-007（TODO → DOING）
- [x] **[流程]** 创建分支 feature/interrupt-resume，初始化 status 文件
- [x] **[文档]** 阶段 A：openspec/specs/architecture/interrupt-and-cancellation.md 初稿 + 定稿（验收表填实际用例名）
- [x] **[流程]** impact-scan：ripgrep 扫描 abort_signal / cancelled / AgentLoop::run 调用面（结论见末尾"impact-scan 结论"）

#### 实施类
- [x] **[P0]** types-refactor：types.rs 引入 CancellationToken / 重塑 LoopError::Aborted 携带 partial / 新增 AgentRunOutcome 三态
- [x] **[P0]** run-select：run.rs 把 LLM stream + 工具执行的 await 改为 tokio::select!，取消时累积 partial → Aborted
- [x] ~~**[P0]** primitive-bash-cancel：PrimitiveExecutor::execute_bash 直改签名增 CancellationToken；select + Child::kill~~ → **决策调整**：不改 trait 签名，caller 端 `tokio::select!` + `Command::kill_on_drop(true)` 已足够
- [x] **[P0]** chat-loop-interrupt-branch：chat_loop 把 Interrupted 走与 Completed 同一持久化路径；token 在 readline 后重建
- [x] **[P0]** ctrlc-double-tap：chat_cmd.rs 抽 check_double_tap 纯函数；首击 cancel，2s 内再击 exit(130)
- [x] **[P1]** events：AgentEvent::Interrupted + WIRE_AGENT_INTERRUPTED；保留 AgentEnd.error="interrupted" 兼容

#### 验收类
- [x] **[P0]** tests：单测覆盖 check_double_tap（4 用例）/ `run_interrupt_between_tools_retains_completed_tool_result` / `run_interrupt_during_stream_preserves_partial_text` / `token_rebuild_per_turn_allows_next_run` / T-017 硬验收 `interrupt_persists_transcript_hard_ack`
- [x] **[P0]** integration-e2e：E2E 场景库 E2E-CLI-062 `test_user_interrupt_during_bash` / E2E-CLI-063 `test_user_double_ctrlc_exits` 已登记到 `E2E_SCENARIO_LIBRARY.md` Story 8（标注人工验收，数据契约由单测锁死）；`cargo test -j 1 --lib` 432 passed / `--test '*'` 全绿（见末尾"测试门禁 2026-04-23"）
- [x] **[P0]** docs-sync：TODOS.md T-003 / T-004 / T-017 标 [x] 附源码锚点 + agent-loop.md §13.2 / §13.3.2 修订（`cancel_token` + Aborted 语义段）+ interrupt-and-cancellation.md 定稿
- [x] **[流程]** flow-self-check：完成前自检；看板 DOING → PENDING_INTEGRATION（本次提交）

### 🔌 INTERFACE (接口变更 / 已落地)

- **`LoopError::Aborted { partial_text: String, partial_messages: Vec<ChatMessage> }`**（破坏性字段变更；内部仅 `run.rs` 4 处 match 同步更新）
- **`AgentRunOutcome`**（新枚举，`Completed` / `Interrupted` / `Failed`）——`AgentLoop::run` 返回类型由 `Result<AgentRunResult, AppError>` 改为 `AgentRunOutcome`；附带 `unwrap` / `unwrap_err` / `is_ok` / `is_err` / `is_interrupted` 便利方法
- **`AgentLoop::new` 签名**：`abort_signal: Arc<AtomicBool>` → `cancel_token: CancellationToken`（破坏性；测试与 `chat_loop` 同步更新）
- **`PrimitiveExecutor::execute_bash` trait**：**未改签名**（decision 2026-04-22，见 impact-scan）；通过 caller 端 `tokio::select!` + `tokio::process::Command::kill_on_drop(true)` 达成可取消语义
- **`AgentEvent::Interrupted { session_id, partial_text_len, tool_results_count }`**（新增 variant，`#[serde(rename = "agent_interrupted")]`）+ `WIRE_AGENT_INTERRUPTED = "agent_interrupted"` 常量
- **`AgentEnd { error: "interrupted" }`**（保留兼容，旧订阅者无需改动）
- **`tokio-util` crate**（新增依赖，features = `["rt"]`）——引入 `CancellationToken`

### ⚠️ BLOCKED (阻塞 / 风险)

| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 依赖偏离 | 看板标 T2-P0-007 依赖 T2-P0-001（AgentLoop 拆分）+ T2-P0-003（Stream timeout），两者均 TODO；本次破例先做（理由见计划 §0.2） | 由 Nibbles 复核是否接受破例 |

### 📌 备注

- 阶段 A（仅文档）已完成，进入阶段 B（代码实施）。
- 提交策略：每子任务完成提交一次（commit-guard），本地 + 远端同步推送。

### 🔍 impact-scan 结论（2026-04-22）

| 改动点 | 调用方 / 实现方 | 处理方式 |
|---|---|---|
| `AgentLoop::run` 返回类型 → `AgentRunOutcome` | `src/api/chat/mod.rs:384`（生产唯一）+ ~6 处 tests | 全部更新 match 模式 |
| `LoopError::Aborted` → 携带 partial | `run.rs` 内 4 处构造 + 4 处 match | 仅 run.rs 自身 |
| `PrimitiveExecutor::execute_bash` trait | 1 处真实现 + 6 处 mock + 0 个外部 plugin | **决策调整**：不改 trait 签名。`tokio::process::Command::output().await` + `.kill_on_drop(true)` 已支持"future drop 即 kill 子进程"。caller 端 `tokio::select!` 包裹即可，0 处 mock 受影响 |
| `AgentLoop::new` 签名（abort_signal 参数） | ~9 处生产 + 测试 | **决策**：保留 abort_signal 参数；内部新增 `cancel_token` 字段，`abort()` 同时 cancel token。0 处调用方改动 |
| `AgentEvent::Interrupted` + `WIRE_AGENT_INTERRUPTED` | events.rs / wire.rs 新增 | 附加而非替代 |
| 新增依赖 `tokio-util` (sync feature) | Cargo.toml | 引入 `CancellationToken` |

**整体结论**：影响面较 plan §6.3 预估缩小（不改 trait 签名节省 6 处 mock 改动 + 0 外部 plugin 影响）。架构文档 `interrupt-and-cancellation.md` §6.3 已在 docs-sync 阶段同步修订该决策调整。

### 🧪 测试门禁（2026-04-23）

| 门禁项 | 命令 | 结果 |
|---|---|---|
| 构建 | `cargo build --all-targets` | ✅ OK |
| 静态检查 | `cargo clippy --all-targets --all-features -- -D warnings` | ✅ 0 warning |
| 格式 | `cargo fmt --all -- --check` | ✅ OK |
| 单元测试 | `RUST_LOG=... cargo test -j 1 --lib -- --nocapture --test-threads=1` | ✅ 432 passed / 0 failed / 1 ignored |
| 集成 + E2E | `RUST_LOG=... cargo test -j 1 --test '*' -- --nocapture --test-threads=1` | ✅ 全部 test suite 通过（cli_tests 77 / wasmedge_e2e_tests 39 等） |

- LLM / 真 API 相关 E2E：`.env` 中 `OPENAI_API_KEY` 与代理已配置，`cli_tests` 77 条全绿（`finished in 80.34s`）。
- Wasm 真实运行时：`wasmedge_e2e_tests` 39 条全绿（`finished in 78.64s`）。
- 人工观感（E2E-CLI-062 / E2E-CLI-063 的 Ctrl+C 体验）建议 Nibbles 在合入 develop 后按 `INTEGRATION_MERGE_AND_ACCEPTANCE.md` §4 人工清单抽验。
