### 元数据

| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Spike | 2026-03-21 | PENDING_INTEGRATION | feature/plugin-compat-phase0 | - |

### 任务

- [✓] **[P1]** TASK-05a Phase 0：pi-mono 插件兼容性技术验证与差距分析
- [✓] 文档：`INTEGRATION_MERGE_AND_ACCEPTANCE` §1–§4 重编号与中性化、Nibbles §4 精简、工作流与 Constitution/PLAN_SPEC/UNIT_TEST_SPEC 引用对齐
- [✓] 集成/E2E：`./scripts/run-integration-tests.sh all` 全量通过；长生命周期 VM 会话结束路径（`cleanup_instance` 先发 `__shutdown`、`__pi_start_event_loop` 退出时 neutralize 定时器）与场景库 [E2E-WASM-033](../openspec/specs/guides/testing/E2E_SCENARIO_LIBRARY.md) 对齐

### 子项进度

| 子项 | 状态 |
| :--- | :--- |
| a.1 pi-mono 工作树 | DONE（本地 Tomcat/pi-mono 非浅克隆；根目录 `tsgo --noEmit` 存在既有上游类型错误，与 Phase 0 无关） |
| a.2 wasmedge-quickjs modules | DONE（`assets/modules/` + `./modules` preopen + E2E） |
| a.3 SWC ts_compiler | DONE |
| a.4 tps POC | DONE（`wasmedge_e2e_tps_transpile_run_script_poc`） |
| a.5 差距分析文档 | DONE（`docs/reports/extension_api_gap_analysis.md`） |
| a.6 扩展矩阵 | DONE（`docs/reports/extension_compat_matrix.md`，15 行采样） |

### INTERFACE

- `transpile_typescript` / `transpile_pi_plugin_for_quickjs`（crate 根 re-export）
- `PI_WASM_QUICKJS_MODULES_PATH`：可选覆盖 Node 兼容模块目录

### BLOCKED

| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |

---

### 元数据（历史）

| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Spike | 2026-03-20 14:00 | ACTIVE | feature/plugin-compat-phase0 | - |

### 任务

- [ ] **[P1]** TASK-05a Phase 0：pi-mono 插件兼容性技术验证与差距分析

### 子项进度

| 子项 | 状态 |
| :--- | :--- |
| a.1 pi-mono 工作树 | 进行中 |
| a.2 wasmedge-quickjs modules | 待办 |
| a.3 SWC ts_compiler | 待办 |
| a.4 tps POC | 待办 |
| a.5 差距分析文档 | 待办 |
| a.6 扩展矩阵 | 待办 |

### INTERFACE

- 无显著变更（完成后补充 `transpile_typescript` 等对外 API）

### BLOCKED

| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
