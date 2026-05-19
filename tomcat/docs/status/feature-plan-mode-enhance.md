# feature/plan-mode-enhance — Status

> 三卡同分支交付：**T2-P1-002 / T2-P1-003 / T2-P1-004**
> 计划单一事实来源：[`~/.cursor/plans/plan_三卡单分支_7e09fef1.plan.md`]
> Dispatcher 偏离说明：本次按用户要求 **三卡同领、同分支**，与默认「一次一卡」不同。

---

## 当前状态

| 字段 | 值 |
|------|------|
| 负责人 | Tom |
| 状态 | DOING |
| 分支 | `feature/plan-mode-enhance` (from `develop`) |
| 起点 commit | (待写入首个 commit 后回填) |
| 阶段 | P4 RV+CP-D 完成 → P5 AQ 进行中 |

## Phase 进度

- [x] **P0** 认领三卡、切分支、README/任务卡/status 更新
- [x] **P0.5** 横切前置（依赖 `serde_yaml` / `AgentLoopConfig` + `SubagentType` / 4 plan 工具进 catalog / transcript 事件 type 常量 / `[plan]`+`[reviewer]` config / `gen-tool-catalog` + `tool_catalog_doc` 回归）
- [x] **P1** PR-PLA — /plan 命令、PlanMode、catalog、recover(stub)、user prefix（+ §9.3A P1 单测 38 个全绿；recover 真正生效随 P2 file_store 补齐）
- [x] **P2** PR-PLB — file_store、ops、tools/{create_plan,update_plan,todos}（stub review，P4 接入）+ §9.3B 单测 38 个全绿（write_plan 原子写/锁/超时；ops 单 in_progress / id 唯一；mode 守卫 / 跨 session 规则 / 自动 completed）
- [x] **P3** MA — AgentRegistry、spawn_subagent_internal、events、CascadeAbort + §9.3D P3 单测 11 个全绿（panic 隔离、三道闸门、cascade abort BFS、RegistrationGuard balanced）
- [x] **P4** RV+CP-D — review.rs + ReviewerDispatcher trait + PlanRuntime::dispatch_reviewer + create_plan::execute_with_reviewer + §9.3D 余量单测 19 个全绿（parse 严格 / 多块取最后 / aborted 路径 / lock 先释放 / round 计数 warning）
- [ ] **P5** AQ — ask_question + CliAskQuestionPanel
- [ ] **P6** PR-PLC — /plan build 五件事 + 原子回滚 + E2E-PLAN-001
- [ ] **P7** PR-PLD/PLE/PLF — TodosPanel、milestone ckpt、cancel→pending、raw edit 拦截、/restore 联动
- [ ] **P8a** 扫尾单测（D1–D12 防御路径）
- [ ] **P8b** `plan_runtime_integration_tests` 全绿 + tokio::time::timeout(30s)
- [ ] **P8c** `plan_cli_e2e` + E2E_SCENARIO_LIBRARY E2E-PLAN-001～016
- [ ] **P8d** gen-tool-catalog + integration/all EXIT_CODE=0 + 人工 PLAN-UX-01～04
- [ ] **done** 三卡子项勾选、PENDING_INTEGRATION、push

## 关键决策

- `ask_question` 因 T2-P0-008 (TUI) 仍 TODO，采用 **CLI MVP**（`readline` + `spawn_blocking`）；IDE 侧为 trait stub
- reviewer / `dispatch_agent` 共用 `AgentRegistry::spawn_subagent_internal`
- 测试 hang 防御：所有 L1/L2 async `tokio::time::timeout(30s)` 包裹；L3 子进程 `kill_on_drop` + 120s 上限
- 测试稳定性：默认 MockLlm/mock HTTP；真 LLM 用例 `#[ignore]`

## 提交日志

(每个 Phase 完成后追加一行 `<commit> — <phase>`)
