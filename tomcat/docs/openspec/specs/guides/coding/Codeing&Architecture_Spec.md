这份规范旨在为 `tomcat` 项目建立深度架构共识。它不仅告诉 AI “怎么写”，更解释了“为什么要这么写”，通过**理论指引 + 代码实践**的方式，确保项目随着复杂度增加依然保持可维护性。

### 关联子规范

本文档为架构级主规范，以下子文档提供各领域的具体规则：

| 子规范 | 定位 |
| :--- | :--- |
| [RUST_IDIOMS_SPEC.md](RUST_IDIOMS_SPEC.md) | Rust 惯用写法与 Clippy 规则速查（Option 组合子、类型别名、数值安全等） |
| [RUST_FILE_LINES_SPEC.md](RUST_FILE_LINES_SPEC.md) | 单文件行数区间、预警与拆分策略（可维护性与 IDE 体验） |
| [COMMENT_SPEC.md](COMMENT_SPEC.md) | 代码注释与 Rustdoc 文档规范 |
| [UNIT_TEST_LAYOUT_SPEC.md](../testing/UNIT_TEST_LAYOUT_SPEC.md) | 单元测试文件组织规范（目录结构、模块挂载） |
| [UNIT_TEST_SPEC.md](../testing/UNIT_TEST_SPEC.md) | 单元测试编写规范（覆盖率、Mock 策略、命名） |
| [INTEGRATION_TEST_SPEC.md](../testing/INTEGRATION_TEST_SPEC.md) | 集成测试编写规范 |
| [COMMIT_MESSAGE_SPEC.md](../workflow/COMMIT_MESSAGE_SPEC.md) | 提交信息格式规范 |
| [STATUS_GUIDE.md](../workflow/STATUS_GUIDE.md) | 进度状态文件规范 |

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
pub mod ext;      // 外部扩展：插件运行时、工具集
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
- **解耦**：如果明天想从一种插件运行时切到另一种实现，只需改 `ext` 层，不影响 `core`。
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

### 错误传播纪律 (Error Propagation Discipline)

以下规则从项目代码审查中沉淀而来，与上述"禁止隐式崩溃"互为补充。

#### 规则 4.1 — 禁止静默丢弃 Result

`let _ = expr_returning_Result` 会将错误完全吞没。调用者无法感知失败，可能导致数据损坏或行为静默降级。

```rust
// BAD — 写入线性内存失败时 JS 侧拿到损坏数据
let _ = memory.set_data(resp_bytes, buf_ptr);

// GOOD — 错误上报，最终反映为 hostcall 失败
memory.set_data(resp_bytes, buf_ptr)
    .map_err(|_| CoreError::Execution(CoreExecutionError::HostFuncFailed))?;
```

若确实不关心返回值（如 `send` 到已关闭的 channel），需用行内注释说明原因：
```rust
let _ = tx.send(msg); // 接收端可能已 drop，此处不影响主流程
```

#### 规则 4.2 — 可恢复跳过必须带日志

当业务逻辑允许跳过某条失败记录（如 JSONL 逐行解析容错）时，**必须通过 `tracing::warn!` 输出原始数据与错误详情**。选择 `warn` 而非 `error`：单条失败不影响整体功能，但需要在日志中留痕，便于排查数据丢失。

```rust
// BAD — 静默吞掉解析错误，数据丢失不可追踪
Err(_) => continue,

// GOOD — 日志包含原始行内容和错误详情
Err(e) => {
    warn!(line = trimmed, error = %e, "skipping unparseable JSONL entry");
    continue;
}
```

#### 规则 4.3 — 第三方 API「伪失败」的处理模式

部分第三方 SDK 在正常退出时也可能返回 `Err`。对这种场景：
1. **必须 `match`**，不得 `let _ =` 或 `unwrap()`
2. **区分真假错误**：通过错误消息模式匹配（如 `contains("exit code 0")`）识别正常退出
3. **附注释说明**：解释为何将某个 `Err` 视为成功，避免后续维护者误改

```rust
match vm.run_func(Some("quickjs"), "_start", []) {
    Ok(_) => Ok(serde_json::Value::Null),
    Err(e) => {
        let msg = e.to_string();
        // QuickJS _start 正常退出时可能返回 "exit code 0" 类 CoreError，不视为失败
        if msg.contains("exit code 0") || msg.contains("success") {
            Ok(serde_json::Value::Null)
        } else {
            Err(AppError::QuickJS(format!("script execution failed: {}", msg)))
        }
    }
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
#[serde(rename_all = "camelCase")] // 与沙箱 ExtensionAPI / JSON 载荷的 camelCase 约定一致
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
插件运行时调用宿主能力（Hostcall）时，如果每个 API 都写一个独立的绑定，会导致 `src/ext/` 变得臃肿且难以维护。
- **集中分发**：采用类似微服务的“路由分发”模式，由一个统一的入口接收调用请求，根据方法名分发给不同的处理器（Processor）。
为了平衡性能与可维护性，遵循以下原则：
- **单一入口多路复用**：宿主仅暴露一个或极少数核心入口（如 `__pi_host_call`），避免桥接面持续膨胀。
- **协议契约**：使用 JSON 序列化参数，与插件侧 JS/TS 生态及 Hostcall 载荷约定一致。
- **异步桥接 (Async Bridge)**：由于 Rust 侧 LLM 调用是 `async` 的，而插件侧调用 often 以同步入口发起，需利用“请求-轮询/回调”模式，确保不阻塞引擎。
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

####  历史 Wasm 边界处理示例（当前 `rquickjs` 不适用）
```rust
// 历史 Wasm guest 场景下的宿主导出原生函数
fn universal_host_handler(
    frame: &CallingFrame, // 历史 Wasm SDK 提供的调用帧
    args: Vec<WasmValue>,
) -> Result<Vec<WasmValue>, HostFuncError> {
    // 理论：通过 frame 获取实例关联的 plugin_id (Context)
    let plugin_id = frame.get_data::<String>().unwrap(); 
    
    let mem_ptr = args[0].to_i32();
    let mem_len = args[1].to_i32();
    
    // SAFETY: 读取内存前必须进行边界校验
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

### 协议完整性 (Protocol Completeness)

Hostcall 协议涉及三个端点——协议文档（`host-call-protocol.md`）、宿主分发器（`dispatcher.rs`）、JS 桥接层（`pi_bridge.js`）。任何一端缺失都会导致调用静默失败。以下规则确保三端始终对齐。

#### 规则 7.1 — 协议文档与代码双向对齐

`host-call-protocol.md` 中定义的每条 `(module, method)` 路由，Dispatcher 的 `dispatch_async` 必须有对应的 match arm。反之，`pi_bridge.js` 中每个 `pi.*` 方法所发起的 hostcall，Dispatcher 也必须能接住。

违反此规则的典型后果：JS 侧调用 `pi.registerCommand()` 发起 `tools.registerCommand`，但 Dispatcher 没有对应路由，请求落入 `_ => Err("unknown method")` 分支，插件功能静默失败且无明显错误提示。

#### 规则 7.2 — 新增协议路由时的检查清单

每次新增或修改 Hostcall 路由时，必须同步完成以下四项，缺一不可：

| 步骤 | 文件 | 动作 |
| :--- | :--- | :--- |
| 1 | `host-call-protocol.md` | 新增/更新 `(module, method)` 定义、参数与返回值说明 |
| 2 | `src/ext/dispatcher.rs` | 在 `dispatch_async` 中添加 match arm + 实现方法 |
| 3 | `assets/js/pi_bridge.js` | 在 `globalThis.pi` 中暴露对应 JS 方法 |
| 4 | `src/ext/dispatcher.rs` tests | 至少 1 个单元测试覆盖新路由的正常路径 |

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

## 9. 并发与锁安全 (Concurrency & Lock Safety)

### 理论 (Theory)

`std::sync::RwLock` / `Mutex` 内置"锁中毒"（poison）机制：持锁线程 panic 后，锁被标记为 poisoned，后续所有 `.read()` / `.write()` 返回 `Err(PoisonError)`。如果用 `.unwrap()` 处理，则**一个线程的 panic 会级联导致所有后续访问 panic**——这是生产环境中不可接受的连锁故障模式。

### 实践 (Practice)

#### 规则 9.1 — 优先使用 `parking_lot` 替代 `std::sync`

项目统一使用 `parking_lot::RwLock` / `parking_lot::Mutex`。parking_lot 没有 poison 机制：持锁线程 panic 后锁正常释放，其他线程可继续工作。这是 Rust 社区主流选择（tokio、servo 等项目均采用）。

```rust
// BAD — std::sync::RwLock + unwrap 级联 panic 风险
use std::sync::RwLock;
let guard = self.tools.write().unwrap();

// GOOD — parking_lot::RwLock，无 poison，API 更简洁
use parking_lot::RwLock;
let guard = self.tools.write(); // 直接返回 guard
```

#### 规则 9.2 — 禁止 `.read().unwrap()` / `.write().unwrap()`

即使在无法使用 parking_lot 的场景（如第三方库约束），也必须处理 `PoisonError`，不得用 `.unwrap()` 绕过。

#### 规则 9.3 — 锁选型指南

| 场景 | 推荐 | 理由 |
| :--- | :--- | :--- |
| 读多写少（ToolRegistry、EventBus） | `parking_lot::RwLock` | 读锁无互斥，写锁独占 |
| 写频繁或持锁时间短 | `parking_lot::Mutex` | 比 RwLock 开销更低 |
| 高并发、大量 key 分片 | `dashmap::DashMap` | 分段锁，避免全局争用 |
| 跨 await 持锁 | `tokio::sync::RwLock` | 不阻塞 tokio 运行时 |

---

## 10. Dead Code 与预留代码管理 (Dead Code Management)

### 理论 (Theory)

Rust 编译器默认对未使用的代码发出 `dead_code` 警告。但在迭代式开发中，部分代码是**有意预留**的（如 stub 模块、未来特性字段、预留的公共基座类型）。需要区分"遗漏的废弃代码"与"有意预留的代码"，并用一致的标注方式表达意图。

### 实践 (Practice)

#### 规则 10.1 — `#[allow(dead_code)]` 必须附带 doc comment

裸 `#[allow(dead_code)]` 无法区分"有意预留"还是"忘记删除"。必须在上方用 `///` 说明预留用途和预计启用时机。

```rust
// BAD — 看不出是预留还是废弃
#[allow(dead_code)]
pub struct EntryBase { ... }

// GOOD — doc comment 说明意图
/// 公共基座：id、parentId、timestamp，预留供后续树形操作使用。
#[allow(dead_code)]
pub struct EntryBase { ... }
```

#### 规则 10.2 — stub/mock 模块用模块级标注

当整个模块是测试辅助或未来保留实现时，可在 `mod.rs` 的模块声明处统一标注 `#[allow(dead_code)]`，避免模块内部每个函数逐条标注。

```rust
// src/ext/tests/mod.rs
#[allow(dead_code)]
mod fixtures;
```

#### 规则 10.3 — 未来特性字段用双重标注

结构体中为未来特性预留的字段，使用 `/// TODO:` + `#[allow(dead_code)]` 双重标注。`TODO` 说明实现方向，`allow` 消除编译警告。

```rust
/// TODO: 接入 tokio::time::timeout 实现流式超时
#[allow(dead_code)]
stream_timeout_sec: u64,
```

---

## 11. 代码复用与去重 (Code Reuse & DRY)

### 理论 (Theory)

DRY（Don't Repeat Yourself）不仅是风格偏好，而是**架构级约束**。重复的结构体、常量或逻辑在演进中不可避免地分叉——一处改了另一处忘了，造成数据不一致、行为分裂。代码复用的核心收益：

- **单一事实来源（Single Source of Truth）**：同一语义只存在一个权威定义。字段增删只改一处，编译器自动传播。
- **类型安全传播**：复用结构体而非散装字段时，新增/移除/重命名字段会产生编译错误，强制所有消费端同步更新。
- **认知负担最小化**：开发者只需理解一个类型的语义，不必在多个"长得像"的定义之间做心智映射。

判断是否需要复用的决策标准：
1. **同构即复用**：如果两个位置的字段集完全一致或构成子集关系，**必须**复用而不是拷贝。
2. **语义一致即复用**：即使字段名略有差异，只要表达相同业务概念（如 `ContextMetrics` 与 `ContextMetricsUpdate` 事件字段），应以一方为权威源，另一方通过 `From` 转换或直接持有。
3. **三次出现即提取**：同一逻辑片段（函数体/匹配模式/构造块）出现三次或以上，必须提取为函数、宏或 trait 方法。

### 实践 (Practice)

#### 规则 11.1 — 同构结构体禁止散装替代

当项目中已存在与需求字段完全对齐的结构体时，**必须**直接持有该类型，禁止将其字段拆散为多个独立变量。

```rust
// BAD — 散装字段是 ContextMetrics 的逐字拷贝，新增字段时必然遗漏
pub struct AgentLoop {
    compaction_count: u32,
    compaction_tokens_freed: usize,
    total_tool_result_bytes_persisted: usize,
    // ...
}

// GOOD — 复用已有类型，字段增删由编译器保障一致性
use crate::core::context_metrics::ContextMetrics;

pub struct AgentLoop {
    metrics: ContextMetrics,
    // ...
}
```

#### 规则 11.2 — 事件/DTO 与内部模型对齐

当事件枚举变体（如 `AgentEvent::ContextMetricsUpdate`）与内部结构体（如 `ContextMetrics`）字段同构时，构造事件应直接从内部结构体取值，避免中间变量二次搬运导致映射错位。

```rust
// BAD — 手动逐字段搬运，新增字段时容易遗漏
self.emit_event(AgentEvent::ContextMetricsUpdate {
    input_tokens_used: local_var_a,
    context_utilization_ratio: local_var_b,
    // 漏了 compaction_count ...
});

// GOOD — 累计在 session_obs，瞬时在 live；无 AgentLoop 侧第二份 metrics
if let Some(ref ctx_state) = self.context_state {
    self.emit_event(AgentEvent::ContextMetricsUpdate {
        input_tokens_used: ctx_state.live.input_tokens_used,
        context_utilization_ratio: ctx_state.live.context_utilization_ratio,
        compaction_count: ctx_state.session_obs.compaction_count,
        compaction_tokens_freed: ctx_state.session_obs.compaction_tokens_freed,
        total_tool_result_bytes_persisted: ctx_state.session_obs.tool_result_chars_persisted,
        preheat_in_progress: ctx_state.live.preheat_in_progress,
        preheat_result_pending: ctx_state.live.preheat_result_pending,
    });
}
```

#### 规则 11.3 — 重复逻辑提取为函数

同一代码模式出现三次或以上时，必须提取为命名函数或宏。提取后函数应放在**语义最近的模块**中（而非全局 utils）。

```rust
// BAD — 相同的 "chars / 4" 近似 token 计算散落在三处
let tokens_a = some_chars / 4;
// ... 另一处 ...
let tokens_b = other_chars / 4;

// GOOD — 提取为具名函数，语义清晰且修改集中
pub(crate) fn estimate_tokens(chars: usize) -> usize {
    chars / 4
}
```

#### 规则 11.4 — 常量与魔数统一管理

相同的阈值、配置默认值、格式字符串出现在两处及以上时，必须提取为命名常量。常量放在语义所属模块内，通过 `pub(crate)` 暴露。

```rust
// BAD — 魔数 4096 散落在 compaction.rs 和 agent_loop.rs
if chars > 4096 { ... }

// GOOD
pub(crate) const LARGE_RESULT_THRESHOLD_CHARS: usize = 4096;
if chars > LARGE_RESULT_THRESHOLD_CHARS { ... }
```

---

## 12. 资源配额与超时控制规范 (Resource Quotas)

### 理论 (Theory)
插件是不可信的（Untrusted Code）。除了权限隔离，还必须进行资源隔离：
- **内存限额**：防止插件申请巨大内存导致宿主 OOM（Out of Memory）。
- **执行耗时**：防止插件死循环（Infinite Loop）挂起宿主进程。
- **原语频率**：防止插件高频调用 `executeBash` 导致拒绝服务攻击。

### 实践 (Practice)
在配置插件运行时时，强制注入资源限制。

```rust
// src/ext/engine_config.rs
pub fn create_secure_vm_config(cfg: &PluginConfig) -> PluginEngineConfig {
    let mut config = PluginEngineConfig::default();
    
    // 1. 限制 JS 堆大小
    config.js_heap_mb = cfg.js_heap_mb;
    
    // 2. 注入单次调用超时与中断预算
    config.call_timeout_ms = cfg.call_timeout_ms;
    config.interrupt_budget = cfg.interrupt_budget;

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
> 1. **这个功能是否会影响 Wasm 插件或 ExtensionAPI 行为？** -> 是：优先对照 `docs/architecture/plugin-system/` 与现有 Hostcall 契约；否：按 Rust 最佳实践实现。
> 2. **这个操作是否需要访问系统资源？** -> 是：必须封装在“4原语”或“宿主API层”中，并经过权限校验；否：可以在插件侧纯 JS 实现。
> 3. **这个数据是否需要持久化？** -> 是：对话记录存 JSONL（Append），元数据存 sessions.json（Atomic Write）；否：存内存 State。
> 4. **这个错误是否会暴露给插件？** -> 是：转换为 `PluginError` 并映射为 JS Exception；否：记录在 `tracing` 日志中。
