| Owner | Update Time | State | Branch |
| :--- | :--- | :--- | :--- |
| primitives_tools | 2025-03-06 23:45 | DONE | feature/primitives-tools |

### ✅ DONE (已完成/进行中)
- [✓] **[P0]** T1-P0-005 用户确认与审计扩展点：UserConfirmationProvider、AuditRecorder、TracingAuditRecorder、AllowAllConfirmation/DenyAllConfirmation @2025-03-06
- [✓] **[P0]** T1-P0-005 DefaultPrimitiveExecutor：read_file/list_dir/write_file/edit_file/execute_bash、路径白名单、用户确认、备份与原子写入、审计记录 @2025-03-06
- [✓] **[P0]** T1-P0-005 单测：各原语成功路径、白名单拒绝、用户拒绝确认边界与审计、覆盖率 @2025-03-06
- [✓] **[P0]** T1-P0-006 工具注册中心 DefaultToolRegistry、ToolExecutor 注入、call_tool 与审计 @2025-03-06
- [✓] **[P0]** T1-P0-006 单测：注册/注销/列表/call_tool/卸载插件自动注销、覆盖率 @2025-03-06

### 🔌 INTERFACE (接口变更)
- **UserConfirmationProvider**：core 层 trait，CLI/chat 实现具体交互；AllowAllConfirmation/DenyAllConfirmation 供测试与默认用。
- **AuditRecorder / PrimitiveAuditEntry / ToolAuditEntry**：infra 层审计扩展点；TracingAuditRecorder 默认实现。
- **DefaultPrimitiveExecutor**：依赖 PrimitiveConfig、UserConfirmationProvider、AuditRecorder；与 design CODE_BLOCK_P1_006 一致。
- **PrimitiveExecutor**、**ToolRegistry**、**Tool**：已有 trait 与类型；006 交付 **DefaultToolRegistry**、**ToolExecutor**（由 008 注入执行逻辑）。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
