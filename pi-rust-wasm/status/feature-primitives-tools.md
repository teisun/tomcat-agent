| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| primitives_tools | 2025-03-07 | DONE | feature/primitives-tools | - |

### ✅ DONE (已完成/进行中)
- [✓] **Phase 0** 开发前：已同步 develop（merge）、检查分支、阅读编码规范 @2025-03-07
- [✓] **Phase 1** 摸底：develop 含 api/core(llm,session)/ext(dispatcher,plugin)/infra；005/006 已整合 core/confirmation、executor、primitives、tools(DefaultToolRegistry,ToolExecutor)、infra/audit；ext 有 HostApiDispatcher，008 可注入 PrimitiveExecutor/ToolRegistry
- [✓] **[P0]** T1-P0-005 用户确认与审计扩展点、DefaultPrimitiveExecutor、单测 @2025-03-06
- [✓] **[P0]** T1-P0-006 工具注册中心 DefaultToolRegistry、ToolExecutor、单测 @2025-03-06
- [✓] merge develop 冲突已解决；session 单测 get_leaf_entry_returns_last 临时目录隔离修复

### 🔌 INTERFACE (接口变更)
- **UserConfirmationProvider**：core 层 trait，CLI/chat 实现具体交互；AllowAllConfirmation/DenyAllConfirmation 供测试与默认用。
- **AuditRecorder / PrimitiveAuditEntry / ToolAuditEntry**：infra 层审计扩展点；TracingAuditRecorder 默认实现。
- **DefaultPrimitiveExecutor**：依赖 PrimitiveConfig、UserConfirmationProvider、AuditRecorder；与 design CODE_BLOCK_P1_006 一致。
- **PrimitiveExecutor**、**ToolRegistry**、**Tool**：已有 trait 与类型；006 交付 **DefaultToolRegistry**、**ToolExecutor**（由 008 注入执行逻辑）。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
