| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| @Spike | 2026-04-22 | DEV | feature/interrupt-resume | - |

## T2-P0-007 | interrupt-resume-transcript | 中断 / 恢复 + transcript 完整性

> 看板单：[../../agents/TASK_BOARD_002.md#t2-p0-007--interrupt-resume-transcript--中断恢复--transcript-完整性](../../agents/TASK_BOARD_002.md)
>
> 计划文档：`~/.cursor/plans/interruptible_agent_loop_c77e96ab.plan.md`
>
> 架构文档：[../../openspec/specs/architecture/interrupt-and-cancellation.md](../../openspec/specs/architecture/interrupt-and-cancellation.md)
>
> 关联 TODOS：`#T-003` `#T-004` `#T-007`（最小版） `#T-017`

### 🚧 IN PROGRESS (开发中)

#### 流程类
- [x] **[流程]** 看板认领 T2-P0-007（TODO → DOING）
- [x] **[流程]** 创建分支 feature/interrupt-resume，初始化 status 文件
- [x] **[文档]** 阶段 A：openspec/specs/architecture/interrupt-and-cancellation.md 初稿
- [ ] **[流程]** impact-scan：ripgrep 扫描 abort_signal / cancelled / AgentLoop::run 调用面

#### 实施类
- [ ] **[P0]** types-refactor：types.rs 引入 CancellationToken / 重塑 LoopError::Aborted 携带 partial / 新增 AgentRunOutcome 三态
- [ ] **[P0]** run-select：run.rs 把 LLM stream + 工具执行的 await 改为 tokio::select!，取消时累积 partial → Aborted
- [ ] **[P0]** primitive-bash-cancel：PrimitiveExecutor::execute_bash 直改签名增 CancellationToken；select + Child::kill
- [ ] **[P0]** chat-loop-interrupt-branch：chat_loop 把 Interrupted 走与 Completed 同一持久化路径；token 在 readline 后重建
- [ ] **[P0]** ctrlc-double-tap：chat_cmd.rs 抽 check_double_tap 纯函数；首击 cancel，2s 内再击 exit(130)
- [ ] **[P1]** events：AgentEvent::Interrupted + WIRE_AGENT_INTERRUPTED；保留 AgentEnd.error="interrupted" 兼容

#### 验收类
- [ ] **[P0]** tests：单测覆盖 check_double_tap / 中断工具 / 中断 stream / token 重建 / partial 落盘
- [ ] **[P0]** integration-e2e：tests/integration/interrupt/* + E2E 场景库 test_user_interrupt_during_bash / test_user_double_ctrlc_exits
- [ ] **[P0]** docs-sync：TODOS.md 标 [x] + agent-loop.md §13.2 修订 + interrupt-and-cancellation.md 定稿
- [ ] **[流程]** flow-self-check：完成前自检；看板 DOING → PENDING_INTEGRATION

### 🔌 INTERFACE (接口变更)

> 待实施完成后填充。预期变更：
>
> - `LoopError::Aborted { partial_text, partial_messages }`（新增字段）
> - `AgentRunOutcome` 新枚举（`Completed` / `Interrupted` / `Failed`）
> - `PrimitiveExecutor::execute_bash` 新增 `cancel: CancellationToken` 参数（**破坏性变更**，所有实现方需更新）
> - `AgentEvent::Interrupted` 新增 variant；`WIRE_AGENT_INTERRUPTED` 新增常量
> - `AgentLoop::run` 返回类型 `Result<AgentRunResult, AppError>` → `AgentRunOutcome`

### ⚠️ BLOCKED (阻塞 / 风险)

| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 依赖偏离 | 看板标 T2-P0-007 依赖 T2-P0-001（AgentLoop 拆分）+ T2-P0-003（Stream timeout），两者均 TODO；本次破例先做（理由见计划 §0.2） | 由 Nibbles 复核是否接受破例 |

### 📌 备注

- 阶段 A（仅文档）已完成，待用户审阅 `interrupt-and-cancellation.md` 后进入阶段 B（代码实施）。
- 提交策略：每子任务完成提交一次（commit-guard），本地 + 远端同步推送。
