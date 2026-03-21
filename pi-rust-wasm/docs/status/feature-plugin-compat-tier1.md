### 元数据

| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Jerry | 2026-03-21 | PENDING_INTEGRATION | feature/plugin-compat-tier1 | wasmedge_e2e |

### 任务

- [x] **[P1]** TASK-05b：pi-mono Tier 1 纯事件监听型扩展（实现已推送分支；待 Nibbles 合并入 develop）

### INTERFACE

- `transpile_pi_plugin_for_quickjs`、`PluginManager::read_main_script`（.ts/.tsx）
- `PluginManager::dispatch_session_event`（`event_type` 透传；与 `EventBus` emit 非自动转发；`wire` 与 [events.md](../../openspec/specs/architecture/plugin-system/events.md) 五段工具链一致）
- `WasmInstance::init_vm` 尾部自动 `__pi_start_event_loop`（零修改 pi-mono 插件）
- `HostApiDispatcher::with_ui_notify_counter`（E2E 断言）
- `assets/js/pi_bridge.js`：`__pi_dispatch_event` 内 `ctx.cwd` 回退 `context.getCwd`

### BLOCKED

| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |

---

## PLAN_SPEC 六维度（TASK-05b 执行依据）

> 对应 [PLAN_SPEC.md](../../agents/plan/PLAN_SPEC.md) 第一节；自检见文末。

### 1. 待完成子项（对照 TASK_BOARD b.1–b.6）

| 子项 | 状态 |
| :--- | :--- |
| b.1 `export default function(pi)` 入口 | 完成：`read_main_script` + `build_combined_script` 对 `.ts`/`.tsx` 转译 |
| b.2 handler `(event, ctx)` | 完成：`__pi_dispatch_event` 传 `(eventData, ctx)` |
| b.3 最小 ctx（hasUI、cwd、ui.notify） | 完成：`ctx.cwd` 空时回退 `context.getCwd` |
| b.4 线格式事件名 | 完成：`AgentEvent` 观察向 `tool_execution_*` + `ExtensionEvent` 钩子 `tool_call`/`tool_result` + `events::wire`；`dispatch_session_event` 无出口映射；插件 VM 侧完整接收 EventBus 钩子事件仍属后续桥接 |
| b.5 tps E2E | 完成：`init_vm` 尾部注入 `__pi_start_event_loop` |
| b.6 自动化 | 完成：E2E-WASM-036 + `with_ui_notify_counter` |

### 2. 目标与验收

**目标**：零修改 [tests/fixtures/pi_mono_tps/tps.ts](../../tests/fixtures/pi_mono_tps/tps.ts) 经 SWC 后在长生命周期 VM 中加载；宿主投递 `agent_start`/`agent_end` 后 `ctx.ui.notify` 触发宿主 `uiNotify`。

**验收**：`cargo test -p pi_wasm -- --test-threads=1 --test wasmedge_e2e_tests` 中含 Tier1 用例通过；`rustfmt`/`clippy` 通过；INTEGRATION 脚本按仓库规范复跑。

### 3. 逐子项要点

- **b.1/b.5** 文件：[ts_compiler.rs](../../src/ext/ts_compiler.rs)、[plugin.rs](../../src/ext/plugin.rs)  
  思路：`read_main_script` 对 `.ts`/`.tsx` 调用 `transpile_pi_plugin_for_quickjs`。  
  测试：单元测试已有 tps fixture；E2E 内联转译产物为 `main`。

- **b.2/b.3** 文件：[pi_bridge.js](../../assets/js/pi_bridge.js)  
  思路：`__pi_resolve_cwd`；handler 已 `(eventData, ctx)`。

- **b.4** 文件：[events.rs](../../src/infra/events.rs) `wire` + `AgentEvent` serde；[plugin.rs](../../src/ext/plugin.rs) 仅透传 `event_type`。

- **长生命周期入口**：[instance_wasmedge.rs](../../src/ext/instance_wasmedge.rs) `init_vm` 仅用于 VmActor，尾部追加 `__pi_start_event_loop()`，避免 pi-mono 插件未写循环时脚本立即退出。

- **b.6 / E2E**：[wasmedge_e2e_tests.rs](../../tests/wasmedge_e2e_tests.rs)：`LongLivedVmPluginMain`、`with_ui_notify_counter`、E2E-WASM-036。

### 4. 实施顺序

b.1 转译与 load → init_vm 事件循环注入 → b.3 cwd → b.4 映射 → dispatcher 计数器 → E2E → 场景库 → 看板。

### 5. 风险与降级

- SWC 产出非 `export default function`：已有 wrap 单测；可扩展 needle。  
- WasmEdge 未安装：E2E 按规范 panic，不 `ignore`。

### 6. 集成与 E2E

- 更新 [E2E_SCENARIO_LIBRARY.md](../../openspec/specs/guides/testing/E2E_SCENARIO_LIBRARY.md) **E2E-WASM-036**。  
- 集成测试：`tests/wasmedge_e2e_tests.rs`。

---

## PLAN_SPEC 自检清单

- [x] 子项与待做/已做对照已列  
- [x] 总体目标与验收已写  
- [x] 用户故事/作用/意义在 §3 与 TASK_BOARD 对齐  
- [x] 每子项含文件路径与设计引用  
- [x] 含实现思路与调用链要点  
- [x] 接口依赖与测试要点已列  
- [x] 实施顺序与依赖已写  
- [x] 风险与降级已写  
- [x] E2E 场景库与测试文件已点名  
