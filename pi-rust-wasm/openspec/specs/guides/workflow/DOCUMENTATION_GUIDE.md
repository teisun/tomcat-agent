
---

# 技术模块文档编写规范 (Prompt Spec)

## 1. 目标与定位
本规范旨在统一 `pi_awsm` 项目的模块文档格式。一份合格的文档应具备：
- **可操作性**：开发者看后能立即知道如何对接 API。
- **架构清晰**：阐明模块在全局中的位置及依赖关系。
- **防御性设计说明**：记录并发、错误处理、安全等关键设计决策。

---

## 2. 文档结构模版 (Markdown)

### # [模块名称] 模块说明

#### ## 1. 概述 (Overview)
- **职责**：一句话描述模块解决的核心问题。
- **所在层级**：(如：基础设施层 / 核心逻辑层 / 接入层)。
- **核心文件**：模块对应的核心代码路径。

#### ## 2. 设计方案 (Design Details)
- **设计模式**：使用了哪些模式（如：单例、观察者、装饰器）。
- **关键权衡**：为什么要这么实现（Why vs What）。
- **线程安全/并发**：是否支持 `Send/Sync`，原子性如何保证。

#### ## 3. 核心 API 与数据结构 (API Definitions)
> **要求**：使用代码块列出核心 Trait 或 Struct 定义，并附带关键注释。
- **数据结构**：`pub struct ...`
- **接口声明**：`pub trait ...`
- **错误处理**：描述该模块可能返回的特定错误变体。

#### ## 4. 配置项 (Configuration)
- 列出相关环境变量、配置文件字段及其默认值。

#### ## 5. 交互流程 (Workflow / Sequence)
- 描述典型调用链路（如：初始化 -> 注册监听 -> 事件触发）。

#### ## 6. 示例代码 (Usage Examples)
- 至少包含一个“快速上手”代码示例。

#### ## 7. 验收标准 (Testing & QA)
- 覆盖率要求、关键测试用例、性能指标。

---

## 3. 写作风格要求
1. **精准定位**：明确引用代码中的文件名和具体的符号（Struct/Enum/Fn）。
2. **术语统一**：严禁混用术语（如：一会儿叫 `Plugin`，一会儿叫 `Extension`）。
3. **图文并茂**：对于复杂逻辑，建议使用 Mermaid 画出流程图或类图。
4. **禁止空洞**：不要写“实现了一些错误处理”，要写“通过 `AppError` 枚举统一封装，支持从 `io::Error` 自动转换”。

---

# 示例：《基础设施层说明》

以下是根据上述规范重写的部分示例，你可以对比差异：

# 基础设施层说明 (Infrastructure Layer)

## 1. 概述
- **职责**：为上层模块（Agent, Plugin, Tool）提供统一的错误处理、配置管理、日志记录、跨平台文件操作及解耦的事件总线。
- **设计原则**：最小依赖、强类型约束、零 Panic。

## 2. 核心模块详解

### 2.1 统一错误处理 (AppError)
- **位置**：`src/error.rs`
- **设计**：采用 `thiserror` 实现的枚举，确保错误上下文可追踪。
- **关键变体**：
  - `Primitive(String)`: 原子工具执行异常。
  - `Plugin(String)`: WASM 运行时或插件逻辑错误。
- **最佳实践**：禁止使用 `unwrap()`，对于不可恢复错误使用 `expect("context")`，优先使用 `?` 向上抛出。

### 2.2 配置中心 (AppConfig)
- **加载机制**：
  1. 默认值 (`Default::default()`)
  2. 配置文件 (`config.toml`)
  3. 环境变量 (前缀 `PI_AWSM__`)
- **校验逻辑**：`validate_config` 在系统启动阶段执行，若关键路径（如 `sessions_dir`）无写权限，程序将拒绝启动。

### 2.3 异步事件总线 (EventBus)
- **位置**：`src/event_bus.rs`
- **并发模型**：`DefaultEventBus` 基于 `Arc<RwLock<...>>` 实现，支持跨线程安全访问。
- **核心接口**：
```rust
#[async_trait]
pub trait EventBus: Send + Sync {
    // 注册监听，返回 ID 用于后续注销
    fn on(&self, event: &str, callback: EventCallback) -> EventListenerId;
    
    // 异步分发事件，不阻塞调用方
    async fn emit_async(&self, ctx: EventContext);
    
    // 插件卸载时一键清理，防止内存泄漏
    fn remove_plugin_listeners(&self, plugin_id: &str);
}
```

## 3. 交互流程：事件处理
1. **注册**：插件系统启动时，调用 `on` 注册 `extension_loaded` 事件。
2. **触发**：`wasm_plugin` 模块加载新插件，调用 `emit_sync`。
3. **分发**：`EventBus` 根据 `priority` 排序，依次执行回调。
4. **异常处理**：若某个回调 Panic，`EventBus` 会通过 `catch_unwind` 捕获并记录日志，不影响其他监听者。

## 4. 示例：如何添加一个新配置
1. 在 `src/config.rs` 的 `AppConfig` 结构体中添加字段。
2. 在 `validate_config` 函数中增加校验逻辑。
3. 在 `tests/config_test.rs` 中添加环境变量覆盖测试。

---

