| Owner | Update Time | State | Branch |
| :--- | :--- | :--- | :--- |
| infra_agent | 2025-03-04 11:00 | DONE | feature/infra |

### ✅ DONE (已完成)
- [✓] **[P0]** T1-P0-001 项目骨架与基础设施层：Rust 项目初始化、AppError、配置加载/合并/校验、tracing 分级日志、跨平台 platform 工具 @2025-03-03
- [✓] **[P0]** T1-P0-002 全局事件总线：EventBus Trait、DefaultEventBus、on/once/off/emit_sync/emit_async/remove_plugin_listeners、单 listener 错误隔离、优先级、单测覆盖 @2025-03-03
- [✓] **[P0]** AgentEvent / ExtensionEvent 枚举与 Architecture 一致（type snake_case、payload camelCase）
- [✓] 技术文档：`docs/01-infrastructure.md` 已编写并随结构更新
- [✓] 按 COMMENT_SPEC 补充基础设施层代码注释 @2025-03-03
- [✓] 按 Codeing&Architecture_Spec 整理：src/infra/ 分层、pub(crate) mod、lib 门面 re-export，对外 API 不变 @2025-03-04

### 🔌 INTERFACE (接口变更)
- 对外 API 仍通过 `pi_awsm::` 根路径使用（由 `lib.rs` 从 `infra` 层 re-export），无破坏性变更。
- **AppError**：项目统一错误枚举，各层通过 `Result<T, AppError>` 使用；不含 Db 变体。
- **AppConfig / LogConfig / PrimitiveConfig / SecurityConfig**：配置与 `load_config`、`validate_config` 入口。
- **EventBus / DefaultEventBus / EventContext / EventListenerId**：事件总线与 `add_listener`（支持 `plugin_id` 便于卸载时清理）。
- **AgentEvent / ExtensionEvent**：事件枚举；扩展侧事件名 snake_case，payload camelCase。
- **init_logging**、**normalize_path**、**read_file_utf8**、**write_file_atomic**：日志与平台工具。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
