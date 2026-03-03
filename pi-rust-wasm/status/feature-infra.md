| Owner | Update Time | State | Branch |
| :--- | :--- | :--- | :--- |
| infra_agent | 2025-03-03 15:00 | DONE | feature/infra |

### ✅ DONE (已完成)
- [✓] **[P0]** T1-P0-001 项目骨架与基础设施层：Rust 项目初始化、AppError、配置加载/合并/校验、tracing 分级日志、跨平台 platform 工具 @2025-03-03
- [✓] **[P0]** T1-P0-002 全局事件总线：EventBus Trait、DefaultEventBus、on/once/off/emit_sync/emit_async/remove_plugin_listeners、单 listener 错误隔离、优先级、单测覆盖 @2025-03-03
- [✓] **[P0]** AgentEvent / ExtensionEvent 枚举与 Architecture 一致（type snake_case、payload camelCase）
- [✓] 技术文档：`docs/01-infrastructure.md` 已编写

### 🔌 INTERFACE (接口变更)
- **AppError**：项目统一错误枚举，各层通过 `Result<T, AppError>` 使用；不含 Db 变体。
- **AppConfig / LogConfig / LlmConfig / StorageConfig / PluginConfig / PrimitiveConfig / SecurityConfig**：配置结构体与 `load_config`、`validate_config` 入口。
- **EventBus**：Trait 提供 `on`、`once`、`off`、`emit_sync`、`emit_async`、`remove_plugin_listeners`；扩展侧使用字符串事件名（snake_case）。
- **EventContext**：`event_name`、`payload`、`plugin_id`、`priority`；支持 `with_plugin_id`、`with_priority`。
- **DefaultEventBus::add_listener**：支持传入 `plugin_id` 便于插件卸载时清理。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
