这份规范旨在为 `pi_awsm` 项目建立深度架构共识。它不仅告诉 AI “怎么写”，更解释了“为什么要这么写”，通过**理论指引 + 代码实践**的方式，确保项目随着复杂度增加依然保持可维护性。

---

# 编码与架构设计高级规范 (Architecture & Coding Excellence)

## 1. 分层架构与职责分离 (Layered Architecture)

### 理论 (Theory)
软件系统的复杂性通常源于模块间不透明的依赖关系。我们采用**单向依赖的分层架构**：
- **隔离性**：底层模块（Infra）不应感知上层业务（Core/Agent）的存在。
- **稳定性**：越往底层，API 越稳定；越往上层，逻辑越频繁变动。
- **可测性**：通过层间解耦，可以轻易通过 Mock 替代底层实现进行单元测试。

### 实践 (Practice)
禁止将所有文件堆放在 `src/` 根目录。必须按以下逻辑组织：

```rust
// src/lib.rs - 作为门面（Facade），管理顶层模块声明
pub mod infra;    // 基础设施：日志、配置、错误定义
pub mod core;     // 核心逻辑：Agent 决策、会话状态
pub mod ext;      // 外部扩展：WASM 运行时、工具集
pub mod common;   // 公共契约：不依赖任何层的纯类型定义

// 示例：src/infra/mod.rs 内部结构
pub(crate) mod config;
pub(crate) mod error;
pub(crate) mod event_bus;

// 重新导出常用类型，简化上层调用路径
pub use error::AppError;
pub use event_bus::{EventBus, DefaultEventBus};
```

---

## 2. 模块可见性与封装 (Encapsulation Control)

### 理论 (Theory)
遵循**最小特权原则 (Least Privilege)**。Rust 的 `pub` 过于强大，一旦暴露，修改成本巨大。
- 内部细节应使用 `pub(crate)`，限制在当前 Crate 内可见。
- 只有模块对外的“契约”才使用 `pub`。

### 实践 (Practice)
在子模块中隐藏实现细节，仅在 `mod.rs` 中通过 `pub use` 暴露必要接口。

```rust
// src/infra/config.rs
pub(crate) struct InnerConfigLoader; // 外部不可见

impl InnerConfigLoader {
    pub(crate) fn load() -> RawData { ... }
}

/// 对外暴露的配置对象 (Doc Comment)
pub struct AppConfig { 
    pub(crate) storage_path: PathBuf 
}

// src/infra/mod.rs
mod config;
pub use config::AppConfig; // 外部只能看到 AppConfig，看不到 InnerConfigLoader
```

---

## 3. 依赖反转与 Trait 抽象 (Dependency Inversion)

### 理论 (Theory)
核心逻辑不应直接依赖具体的第三方库或底层实现。通过 **Trait (接口)** 定义需求，由底层实现这些 Trait。
- **解耦**：如果明天想从 `WasmEdge` 换成 `Wasmtime`，只需改 `ext` 层，不影响 `core`。
- **Mocking**：在测试 Agent 时，可以传入一个内存中的 `MockDatabase` 而非真实的 SQLite。

### 实践 (Practice)
在 `core` 或 `common` 中定义 Trait，在 `infra` 或 `ext` 中实现。

```rust
// src/core/traits.rs - 定义核心所需的契约
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn completion(&self, prompt: &str) -> Result<String, AppError>;
}

// src/ext/llm/openai.rs - 具体的底层实现
pub struct OpenAiProvider { ... }

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn completion(&self, prompt: &str) -> Result<String, AppError> {
        // 调用具体的 OpenAI API
    }
}
```

---

## 4. 健壮的错误处理 (Robust Error Handling)

### 理论 (Theory)
错误处理不仅是捕获异常，更是**上下文管理**。
- **内部错误**：使用 `thiserror` 定义具体的枚举，提供类型安全的分类。
- **业务上下文**：使用 `anyhow` 或 `error_stack` 在错误向上抛出时附加“是在处理哪个用户请求时崩溃”的信息。
- **禁止隐式崩溃**：严禁在生产代码中使用 `unwrap()`，必须显式处理或通过 `expect` 附带原因。

### 实践 (Practice)
```rust
// src/infra/error.rs
#[derive(thiserror::Error, Debug)]
pub enum AppError {
    #[error("Config file not found at {0}")]
    ConfigNotFound(String),
    
    #[error("Plugin {id} execution timeout after {secs}s")]
    PluginTimeout { id: String, secs: u64 },

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

// 业务调用逻辑
pub fn load_plugin(id: &str) -> Result<(), AppError> {
    let raw = std::fs::read(path).map_err(|e| anyhow::anyhow!("Failed to read {id}: {e}"))?;
    // ...
    Ok(())
}
```

---

## 5. 防御性设计与路径规范 (Defensive Programming)

### 理论 (Theory)
作为 Agent 系统，安全是第一优先级。
- **输入校验**：所有来自 LLM 或外界的路径、命令必须在边界处进行规范化。
- **原子性**：文件操作应优先使用“先写临时文件再重命名”的模式，防止写入中断导致文件损坏。

### 实践 (Practice)
```rust
// src/infra/platform.rs
pub fn safe_write_file(path: &Path, content: &[u8]) -> Result<(), AppError> {
    // 1. 规范化并校验路径是否越权（防止 ../../）
    let safe_path = normalize_and_verify(path)?;
    
    // 2. 创建临时文件
    let temp_path = safe_path.with_extension("tmp");
    std::fs::write(&temp_path, content)?;
    
    // 3. 原子重命名
    std::fs::rename(temp_path, safe_path)?;
    Ok(())
}
```

---


## 6. 跨语言边界契约规范 (Inter-Language Contract)

### 理论 (Theory)
由于本项目涉及 Rust（宿主）与 JS/TS（插件）的频繁交互，数据序列化的性能损耗和字段命名冲突是主要风险。
- **命名一致性**：遵循“内部 Rust 惯用，边界严格对齐”原则。
- **零拷贝倾向**：在处理大文件（Read/Write 原语）时，应优先考虑字节流或内存映射，避免将巨大文件直接转为 JSON 字符串。
- **Schema 权威**：Rust 定义的 `struct` 是 Source of Truth，通过 `serde` 属性自动适配 JS 端的 `camelCase`。

### 实践 (Practice)
所有跨边界传输的 DTO（数据传输对象）必须显式标注序列化规则。

```rust
// 位于 src/common/dto.rs
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")] // 强制对齐 pi-mono 的 JS 命名规范
pub struct ToolCallRequest {
    pub tool_name: String,
    pub call_id: String,
    pub arguments: serde_json::Value, // 通用参数容器
}

// 在 Wasm 导入函数中使用时
pub fn host_register_tool(req_json: String) -> Result<(), AppError> {
    // 理论：在边界处立即进行强类型转换
    let req: ToolCallRequest = serde_json::from_str(&req_json)
        .map_err(|e| AppError::Serialize(e.to_string()))?;
    // ... 执行逻辑
}
```

---

## 7. 插件 Hostcall 分发规范 (Hostcall Dispatching)

### 理论 (Theory)
Wasm 沙箱调用宿主能力（Hostcall）时，如果每个 API 都写一个独立的绑定，会导致 `src/ext/wasm/` 变得臃肿且难以维护。
- **集中分发**：采用类似微服务的“路由分发”模式，由一个统一的入口接收调用请求，根据方法名分发给不同的处理器（Processor）。
为了平衡性能与可维护性，遵循以下原则：
- **单一入口多路复用**：宿主仅向 Wasm 注册一个或极少数核心 Import 函数（如 `__pi_host_call`），避免 Wasm 导入表臃肿。
- **协议契约**：使用 JSON 序列化参数。完美对齐 `pi-mono` 的 JS 生态。
- **异步桥接 (Async Bridge)**：由于 Rust 侧 LLM 调用是 `async` 的，而 Wasm 调用通常是同步语义，需利用 WasmEdge 的异步转译或“请求-轮询/回调”模式，确保不阻塞引擎。
- **上下文感知**：每次调用必须自动关联插件 ID，用于权限校验和审计日志。

### 实践 (Practice)
####  数据契约 (DTO)
```rust
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HostRequest {
    module: String,  // 如 "fs", "agent"
    method: String,  // 如 "readFile", "sendMessage"
    params: serde_json::Value,
    call_id: String, // 异步追踪 ID
}
```

####  宿主分发器 (Dispatcher)
```rust
// src/ext/wasm/dispatcher.rs
pub struct HostApiDispatcher {
    plugin_id: String, // 自动关联当前插件
}

impl HostApiDispatcher {
    /// 核心分发逻辑：支持异步 Processor
    pub async fn dispatch(&self, req: HostRequest) -> Result<serde_json::Value, AppError> {
        // 1. 权限校验 (例如：该插件是否有权调用该 module)
        self.check_permission(&req.module, &req.method)?;

        // 2. 路由分发
        match req.module.as_str() {
            "fs" => FsProcessor::handle(&self.plugin_id, &req.method, req.params).await,
            "agent" => AgentProcessor::handle(&self.plugin_id, &req.method, req.params).await,
            _ => Err(AppError::ApiNotFound(req.module)),
        }
    }
}
```

####  Wasm 边界处理 (Low-level Handler)
```rust
// 宿主导出的 Wasm 原生函数
fn universal_host_handler(
    frame: &CallingFrame, // WasmEdge 提供的调用帧
    args: Vec<WasmValue>,
) -> Result<Vec<WasmValue>, HostFuncError> {
    // 理论：通过 frame 获取实例关联的 plugin_id (Context)
    let plugin_id = frame.get_data::<String>().unwrap(); 
    
    let mem_ptr = args[0].to_i32();
    let mem_len = args[1].to_i32();
    
    // SAFETY: 读取内存前必须进行边界校验 (WasmEdge SDK 通常已处理)
    let json_data = read_wasm_memory(frame, mem_ptr, mem_len)?; 
    let request: HostRequest = serde_json::from_slice(&json_data)?;

    // 运行异步任务并等待结果 (使用当前 Tokio 句柄)
    let dispatcher = HostApiDispatcher { plugin_id };
    let result = tokio::runtime::Handle::current().block_on(dispatcher.dispatch(request));

    // 将结果写回并返回指针
    let (ret_ptr, ret_len) = write_to_wasm_and_manage_lifecycle(result)?;
    Ok(vec![WasmValue::from_i32(ret_ptr), WasmValue::from_i32(ret_len)])
}
```
---

## 8. 存储原子性与状态一致性规范 (Storage Consistency)

### 理论 (Theory)
项目放弃了 SQLite，改用 `sessions.json` + `JSONL`。这种方案的风险在于**非原子性写入**（如写入 JSONL 成功但更新 sessions.json 失败）。
- **Write-Ahead Pattern**：先追加（Append）不可变的 JSONL 对话记录，再更新（Overwrite）可变的元数据 store。
- **异常恢复**：系统启动时应具备简单的修复能力，例如扫描 `sessions/` 目录，若 `sessions.json` 缺失，尝试根据 `.jsonl` 的 `SessionHeader` 重建索引。

### 实践 (Practice)
```rust
// src/core/session/manager.rs
pub async fn save_message(session_id: &str, msg: Message) -> Result<(), AppError> {
    // 1. 获取文件句柄，使用追加模式 (Theory: Append-only is safer)
    let transcript_path = get_transcript_path(session_id);
    infra::platform::append_jsonl(&transcript_path, &msg)?;

    // 2. 更新元数据缓存并持久化
    let mut store = self.load_store()?;
    store.entry(session_id).update_timestamp();
    
    // Theory: 写入临时文件后 Rename，确保 sessions.json 始终完整
    infra::platform::write_file_atomic(STORE_PATH, &store)?;
    Ok(())
}
```

---

## 11. 资源配额与超时控制规范 (Resource Quotas)

### 理论 (Theory)
插件是不可信的（Untrusted Code）。除了权限隔离，还必须进行资源隔离：
- **内存限额**：防止插件申请巨大内存导致宿主 OOM（Out of Memory）。
- **执行耗时**：防止插件死循环（Infinite Loop）挂起宿主进程。
- **原语频率**：防止插件高频调用 `executeBash` 导致拒绝服务攻击。

### 实践 (Practice)
在配置 `WasmEdge` 实例时，强制注入资源限制。

```rust
// src/ext/wasm/config.rs
pub fn create_secure_vm_config(cfg: &PluginConfig) -> WasmEdgeConfig {
    let mut config = WasmEdgeConfig::default();
    
    // 1. 限制线性内存大小 (例如 128MB)
    config.set_max_memory_pages(2048); 
    
    // 2. 开启指令计数（防止死循环）
    config.set_statistics(true);
    config.set_cost_table(...); 

    // 3. 在宿主 API 层实现 Rate Limiting (理论：逻辑限流)
    if self.call_counter.get(plugin_id) > MAX_CALLS_PER_MINUTE {
        return Err(AppError::RateLimitExceeded);
    }
    
    config
}
```

---

## 通用指示：

供 AI 在遇到模糊地带时查阅：

> **架构决策树：**
> 1. **这个功能是否会影响 `pi-mono` 插件运行？** -> 是：优先参考 `pi-mono` 的 API 行为；否：按 Rust 最佳实践实现。
> 2. **这个操作是否需要访问系统资源？** -> 是：必须封装在“4原语”或“宿主API层”中，并经过权限校验；否：可以在插件侧纯 JS 实现。
> 3. **这个数据是否需要持久化？** -> 是：对话记录存 JSONL（Append），元数据存 sessions.json（Atomic Write）；否：存内存 State。
> 4. **这个错误是否会暴露给插件？** -> 是：转换为 `PluginError` 并映射为 JS Exception；否：记录在 `tracing` 日志中。
