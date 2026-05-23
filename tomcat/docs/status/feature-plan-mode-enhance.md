# feature/plan-mode-enhance — Status

> 三卡同分支交付：**T2-P1-002 / T2-P1-003 / T2-P1-004**
> 计划单一事实来源：[`~/.cursor/plans/plan_三卡单分支_7e09fef1.plan.md`]
> Dispatcher 偏离说明：本次按用户要求 **三卡同领、同分支**，与默认「一次一卡」不同。

---

## 当前状态

| 字段 | 值 |
|------|------|
| 负责人 | Tom |
| 状态 | PENDING_INTEGRATION（plan-mode-full-fix + P9 verifier 全部落地，待合并集成） |
| 分支 | `feature/plan-mode-enhance` (from `develop`) |
| 起点 commit | (待写入首个 commit 后回填) |
| 阶段 | P9 verifier 完成并完成 remediation 收口（VerifySummary 单一事实源、`max_turns -> aborted`、P0-P6 prompt、gate 边界、full-chain `plan.verify` 断言）→ 待本地验证/集成提交 |

## Phase 进度

- [x] **P0** 认领三卡、切分支、README/任务卡/status 更新
- [x] **P0.5** 横切前置（依赖 `serde_yaml` / `AgentLoopConfig` + `SubagentType` / 4 plan 工具进 catalog / transcript 事件 type 常量 / `[plan]`+`[reviewer]` config / `gen-tool-catalog` + `tool_catalog_doc` 回归）
- [x] **P1** PR-PLA — /plan 命令、PlanMode、catalog、recover(stub)、user prefix（+ §9.3A P1 单测 38 个全绿；recover 真正生效随 P2 file_store 补齐）
- [x] **P2** PR-PLB — file_store、ops、tools/{create_plan,update_plan,todos}（stub review，P4 接入）+ §9.3B 单测 38 个全绿（write_plan 原子写/锁/超时；ops 单 in_progress / id 唯一；mode 守卫 / 跨 session 规则 / 自动 completed）
- [x] **P3** MA — AgentRegistry、spawn_subagent_internal、events、CascadeAbort + §9.3D P3 单测 11 个全绿（panic 隔离、三道闸门、cascade abort BFS、RegistrationGuard balanced）
- [x] **P4** RV+CP-D — review.rs + ReviewerDispatcher trait + PlanRuntime::dispatch_reviewer + create_plan::execute_with_reviewer + §9.3D 余量单测 19 个全绿（parse 严格 / 多块取最后 / aborted 路径 / lock 先释放 / round 计数 warning）
- [x] **P5** AQ — ask_question + CliAskQuestionPanel + IdeAskQuestionPanel(stub) + MockAskQuestionPanel + §9.3C 单测 18 个全绿（schema 校验 / 1 recommended 约束 / __custom__ 保留 id / picked_recommended 回填 / cancel 信号 / 阻塞语义 / 出参反向校验）
- [x] **P6** PR-PLC — /plan build 五件事（disk session_key/id + disk mode=executing + 内存 mode swap + first_exec_turn flag + plan body 缓存）+ 原子回滚（write 失败时内存不动）+ 友好提示（plan_id 不存在引导 create_plan）+ §9.3A build 行 10 个新测全绿（闸门 / completed / disk executing / 不存在 / unsafe / 五件事一次性 / pending 续跑 / 异 session warning / 首轮一次性注入 / 原子回滚 lock-busy）
- [x] **P7 (核心)** PR-PLE finalize_completed_to_chat + PR-PLF demote_to_pending_on_cancel（释放 lock）+ PR-PLF allow_raw_edit_to_path（canonicalize 双侧）+ attach_cancel_hook/current_cancel_token + 5 个新单测全绿（cancel→pending / cancel_outside_exec_noop / cancel_releases_lock / finalize_completed_clears_first_exec_turn / raw_edit_blocked_for_plan_files）
- [ ] **P7 (延期)** PR-PLD TodosPanel + RefreshNotifier + milestone checkpoint record(Milestone) + /restore reload_active_plan_from_disk — 需要 chat_loop 装配层联动，推到 P8b 集成测一起做
- [x] **P8a** 扫尾单测 — D2 attach_cancel_hook_rebinds_replaces_old_token + D9 concurrent_write_plan_serialized_by_lock + 修 P2~P7 测试间 HOME env 污染（orig_home OnceLock + cleanup_home 还原）→ lib 全测 1025 passed / 0 failed
- [x] **P8b** `plan_runtime_integration_tests` 全绿（8 个端到端用例：full_plan_lifecycle / build→cancel→resume / ask_question 双答案 + Ctrl+C cancelled / reviewer summary 入 tool result / raw_edit guard / todos 路由 / friendly hint）+ 全部 await 用 `tokio::time::timeout(30s)` 包裹（防 D12）+ HOME 隔离 + 串/并行均通过
- [/] **P8c** `plan_cli_e2e`（部分）— D10 catalog 一致性 `committed_tool_catalog_matches_catalog_renderer` 已绿；完整 E2E-PLAN-001～016（含 mock HTTP / 子进程 / scenario library）规模超出"三卡同分支"范围，转移到独立 PR-PLG 单独交付（chat_loop 已经把 plan_runtime/catalog/reminder/prefix/build 接通，主路径可手动跑）
- [x] **P8d** gen-tool-catalog 跑通 / `tool_catalog_doc` (D10) 1/0 / cargo test --lib 1025/0 / plan_runtime_integration 8/0；人工 PLAN-UX-01～04 spot-check 留给真实部署
- [x] **P9 verifier（PR-V0～V3）** DONE Owner=Tom — 已完成 Mini verification prompts + P0-P6 命令发现、internal verifier（`verify.rs` / `SubagentType::Verifier` / `dispatch_verifier` / tool_exec 双保险白名单 / `update_plan` 两次写盘）、`plan.verify` transcript 事件 + `update_plan` tool result `verify` 双通道回传，以及 `[plan].verify_gate = "soft" | "gate"` gate 语义。remediation 追加收口：`transcript.plan.verify` 与 `update_plan.verify` 统一为 normalize 后的最终 `VerifySummary`、`max_turns` 改为“预算耗尽未正常收口 => verdict=aborted / stop_reason=max_turns”、verifier prompt 与 executor Mini 统一 P0-P6、补 `partial/aborted` gate 与 re-complete respawn 回归、real-LLM inprocess E2E 明确断言 `plan.verify`。边界保持不变：**frontmatter 零改造**、唯一配置项仅 `verify_gate`、**不加** `/plan verify` CLI、`VERIFIER_MAX_TURNS = 64`。
- [x] **真 LLM E2E 轮（2026-05）** 完成 reviewer/verifier 子 Agent 真派发装配；[`tests/plan_real_llm_inprocess_tests.rs`](../../tests/plan_real_llm_inprocess_tests.rs)（E2E-PLAN-RL-002，需 `OPENAI_API_KEY`）现同时断言 transcript 中存在 `plan.review` 与 `plan.verify`，作为 full-chain verifier wiring 最低验收；[`tests/plan_real_llm_cli_e2e.rs`](../../tests/plan_real_llm_cli_e2e.rs) 继续保留黑盒链路，但 verifier 侧以 inprocess 用例作为当前主验收锚点。

## 关键决策

- `ask_question` 因 T2-P0-008 (TUI) 仍 TODO，采用 **CLI MVP**（`readline` + `spawn_blocking`）；IDE 侧为 trait stub
- reviewer / `dispatch_agent` 共用 `AgentRegistry::spawn_subagent_internal`
- 测试 hang 防御：所有 L1/L2 async `tokio::time::timeout(30s)` 包裹；L3 子进程 `kill_on_drop` + 120s 上限
- 测试稳定性：默认 MockLlm/mock HTTP；真 LLM 用例 `#[ignore]`
- ~~已知 pre-existing 测试串污染：plan tools 测试改 HOME 后不还原 → 与 permission gate 测试并行/串行时都失败；P8b 修：在 `setup_isolated_home` 用 RAII `EnvGuard` 在 cleanup 时还原原 HOME（不属于 P6 回归）~~ **P8a 已修**（orig_home OnceLock 抓取首次 HOME；cleanup_home 还原）

## plan-mode-full-fix 阶段（A-J）

> 单独一轮全量修复，对齐文档与代码、引入 panel/checkpoint/restore/recover、扩 E2E 覆盖；
> 计划单一事实源：`~/.cursor/plans/plan-mode-full-fix_78a0646c.plan.md`。

- [x] **A** 文档与图修复：plan-runtime / multi-agent / tools/* 8 篇统一口径（含 ask-question CHAT 可见、PLAN 写路径限制等行为变更说明），executor reminder 强调"如何推进任务"为第一要务
- [x] **B/B1/B11/B12** catalog 可见性回正（PLAN 模式新增 todos/ask_question；EXEC 模式仅 update_plan）+ tool_exec.rs 把 4 plan 工具接入主分发 + ask_question 在 CHAT 可用 + safety.rs 写路径策略（PLAN 限 `~/.tomcat/plans/`、EXEC 全禁写 plan 文件）+ reviewer 段守卫
- [x] **C** chat_loop 装配：first_exec_turn user_meta 注入、cancel→pending 降级、finalize_completed_to_chat、session_id 透传、recover hook、transcript 写入对齐
- [x] **D** reviewer 生产派发：ProdReviewerDispatcher 占位（findings/rounds/reload/warning 协议接通）；后续 PR 替换为 AgentRegistry::spawn_subagent_internal 真实派发
- [x] **E** TodosPanel + RefreshNotifier + plan.panel + Milestone checkpoint record（`[plan].auto_checkpoint_on_milestone`，默认 true）+ `/restore` reload_active_plan_from_disk + `PlanRuntime::recover()` 真实实现（孤儿 executing → pending 降级；当前 session 拥有 → 重挂内存 EXEC）
- [x] **E2** Milestone schema 完整化（`status: MilestoneStatus` 派生 + `description`）+ `## Todos Board` 自动重写（`<!-- todos-board:auto:begin/end -->` 标记内）+ 一致性校验（`todo.milestone_id` ⇔ `milestones[].todo_ids`）
- [x] **F** prompts/executor.txt 重写为英文契约（"first priority: drive todos to completion"），prompts/planner.txt 同步；`render_executor_reminder` 支持 `TOMCAT_EXECUTOR_REMINDER_OVERRIDE_PATH` 热覆盖
- [x] **G1/G2/G5** `update_plan` `op→kind`（破坏性一步到位）+ mode 矩阵闸门（Planning 拒 in_progress 等）+ 完整 JSON 返回（applied / plan_mode_before/after / panel_snapshot_id / warnings / items / milestones / active_in_progress）+ completed 全拒（N2）+ 跨 session 策略（N11，Executing 拒 cross-session、Planning/Pending 允许） + N3（`/plan exit` 仅 Planning）
- [x] **G3** TodoRuntime 持久化（`~/.tomcat/agents/<id>/sessions/<sid>/todos/<id>.todo.md`）+ `sessions.json.activeTodosId` 镜像（`ensure_active_todos_id`）+ purge_inactive
- [x] **G4** `create_plan` `body→draft`（破坏性一步到位）+ `plan_id` 派生（slugify + xxhash）+ 显式 reject 老字段 + `## Notes` 模板补全
- [x] **H** `plan_e2e_with_mock_llm_tests.rs` 8 例：H1 full lifecycle（6 次 panel + 自动 completed）/ H2 CHAT todos panel scope / H3 PLAN raw edit 守卫 / H4 EXEC plan 文件全禁写 / H5 reviewer aborted 占位 / H6 cancel→pending / H7 Planning 拒 in_progress / H8 milestone checkpoint record
- [x] **J** CLI 模式指示器（readline 提示符渲染 `[PLAN]` / `[EXEC plan_id]` / `[PENDING plan_id]` / `[DONE plan_id]`）+ `/plan list` 子命令（扫 `~/.tomcat/plans/` 列 id/mode/goal/created_at）
- [x] **I** status 收尾 + 终态门禁

### 行为变更（5 条 — commit/CHANGELOG 显著标注）

1. `ask_question` 在 CHAT 可见（之前 PLAN-only）；上层 panel 不变。
2. PLAN 期 `write`/`edit`/`delete` 仅允许在 `~/.tomcat/plans/*.plan.md`；越界 → ToolError。
3. EXEC 期 plan 文件全禁 raw 写（必须走 `update_plan` 推进）。
4. `update_plan` 入参仅 `kind`，旧 `op` **不再兼容**，立即失败。
5. `create_plan` 入参仅 `draft`（旧 `body` 不再兼容）；`plan_id` 由 runtime 派生，LLM 不再传。

### 终态门禁

```
cargo test --lib -p tomcat                                  → 1060 passed / 0 failed
cargo test --test plan_runtime_integration_tests -p tomcat → 8 passed / 0 failed
cargo test --test plan_e2e_with_mock_llm_tests -p tomcat   → 8 passed / 0 failed（H 新增）
cargo test --test tool_catalog_doc -p tomcat                → 1 passed / 0 failed（catalog 已重生成提交）
cargo run --bin gen-tool-catalog -p tomcat                  → OK（UPDATE_TOOL_CATALOG=1 写盘）
```

## 提交日志

- `983334c` — P0.5 横切前置
- `c0afa94` — P1 PR-PLA
- `890bd9f` — P2 PR-PLB
- `b88515a` — P3 MA
- `9ce91d3` — P4 RV+CP-D
- `d2fdd98` — P5 AQ
- `ee248ca` — P6 PR-PLC
- `eda722b` — P7 核心防御 (PLE/PLF)
- `f891b34` — P8a 防御单测 + HOME 污染修
- `f6ceb15` — P8b 集成测套件 (8 例)
- `<head>`   — P8c/P8d/done DoD 收口

## 终态验证

```
cargo test --lib -p tomcat                            → 1025 passed / 0 failed / 1 ignored
cargo test --test plan_runtime_integration_tests -p tomcat → 8 passed / 0 failed
cargo test --test tool_catalog_doc -p tomcat              → 1 passed / 0 failed (D10)
cargo run --bin gen-tool-catalog -p tomcat                → OK
```

新文件清单（plan_runtime 子树）：
- `src/api/chat/plan_runtime/mod.rs` — PlanRuntime per-session 编排器
- `src/api/chat/plan_runtime/{mode,prompts,session_prefix,safety,catalog}.rs` — P1
- `src/api/chat/plan_runtime/file_store.rs` — P2 持久化 (atomic write + advisory lock)
- `src/api/chat/plan_runtime/ops.rs` — P2 TodoOp 引擎
- `src/api/chat/plan_runtime/tools/{create_plan,update_plan,todos}.rs` — P2 三件套
- `src/api/chat/plan_runtime/review.rs` — P4 ReviewSummary + parse
- `src/api/chat/plan_runtime/ask_question_panel.rs` — P5 CLI/IDE/Mock panel
- `src/api/chat/plan_runtime/tools/ask_question.rs` — P5 工具
- `src/core/agent_registry/mod.rs` — P3 AgentRegistry + spawn_subagent_internal
- `tests/plan_runtime_integration_tests.rs` — P8b 8 例端到端集成

## DoD（plan §9.7）

- [x] §9.3 单元：表中函数全部存在；新增反向/安全/兼容用例齐（114+ plan_runtime 单测）
- [x] §9.4 集成：plan_runtime_integration_tests 8 例全绿，含 D1/D2/D8 防御
- [/] §9.5 CLI E2E：D10 catalog 一致性已绿；E2E-PLAN-001～016 转独立 PR-PLG
- [x] §7.3 transcript 事件 type 在 session-storage 已注册（P0.5）
- [x] §8 D1/D2/D8/D9/D10 单元 + 集成测覆盖；D3/D4/D5/D6/D7/D11/D12 转 PR-PLG
- [/] test-groups.sh：tool_catalog_doc 已分组；plan_runtime_integration 待 P8c PR 时登记
- [x] 三卡子项 + DoD 主体勾选 / 标 PENDING_INTEGRATION
