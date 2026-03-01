# 项目集成与进度看板

## infra

state(状态)：DONE  
branch(分支)：feature/infra  

DONE(完成)：
- [DONE] T1-P0-001 项目骨架与基础设施层：Cargo 项目、AppError、配置结构体与加载/校验、tracing 日志、跨平台 platform 模块
- [DONE] T1-P0-002 全局事件总线：AgentEvent/ExtensionEvent、EventBus Trait、DefaultEventBus（on/once/off/emit_sync/emit_async/remove_plugin_listeners）、单 listener 错误隔离

INTERFACE(接口)：
- AppError、AppConfig/LogConfig/LlmConfig/StorageConfig/PluginConfig/SecurityConfig/PrimitiveConfig、load_config、validate_config
- init_logging(LogConfig)
- normalize_path、read_file_utf8、write_file_atomic、current_dir、system_info
- AgentEvent、ExtensionEvent
- EventBus、DefaultEventBus、EventContext、EventListenerId、EventCallback

**BLOCKED(阻塞性)**：无

### 覆盖率（满足 ≥90% 验收）

- 基础设施核心（排除 main + logging 全局 init）：  
  `cargo tarpaulin --exclude-files "src/main.rs" "src/logging.rs" --out stdout --no-fail-fast`  
  → **99.4%**，满足 ≥90%。
- 提交前建议：`cargo test && cargo tarpaulin --exclude-files "src/main.rs" "src/logging.rs" --out stdout`，将输出中的覆盖率填入 commit message `[cov = xx.x%]`。
