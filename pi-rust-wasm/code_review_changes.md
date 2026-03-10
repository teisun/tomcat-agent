# 代码审查整改——逐条改动说明

> 审查日期：2026-03-09  
> 审查范围：pi-rust-wasm 全量代码  
> 依据规范：Constitution.md、Codeing&Architecture_Spec.md、UNIT_TEST_SPEC.md

---

## 一、P0 关键修复

### 1. Clippy 全量修复（19 条警告 → 0 条）

#### 1.1 `src/infra/logging.rs` — `empty_line_after_doc_comments`

**改了什么**：删除两段 doc comment 之间的空行。

**为什么改**：Clippy 规则 `empty_line_after_doc_comments` 要求 `///` doc comment 与其修饰的 item 之间不能有空行，否则可能被误读为两段不相关的注释。空行会导致第一段注释"悬空"（不附着到任何 item），属于代码风格缺陷。

---

#### 1.2 `src/core/session/transcript.rs` — `dead_code` (EntryBase)

**改了什么**：为 `EntryBase` 结构体添加 `#[allow(dead_code)]`。

```rust
/// 公共基座：id、parentId、timestamp，树形结构。预留供后续树形操作使用。
#[allow(dead_code)]
pub struct EntryBase { ... }
```

**为什么改**：`EntryBase` 定义了 transcript 条目的公共基座字段（id、parentId、timestamp），是架构设计中预留的树形操作基础类型。当前代码尚未直接使用它（各 entry variant 各自定义了相同字段），但删除它会丢失设计意图。加 `#[allow(dead_code)]` 并附带 doc comment 说明用途，明确这是**有意预留**而非遗漏。

---

#### 1.3 `src/core/session/transcript.rs` — `unnecessary_map_or`（2 处）

**改了什么**：

```rust
// Before:
if entry_id(&entry).map_or(false, |s| s == id) {

// After:
if entry_id(&entry) == Some(id) {
```

**为什么改**：`Option<T>.map_or(false, |x| x == value)` 等价于 `Option<T> == Some(value)`，后者更直观、更 Rust 惯用。Clippy 规则 `unnecessary_map_or` 检测这种可简化的模式。改后可读性提升，语义完全等价。

---

#### 1.4 `src/api/cli.rs` — `map_flatten`

**改了什么**：

```rust
// Before:
.map(|s| normalize_path(s).ok())
.flatten()

// After:
.and_then(|s| normalize_path(s).ok())
```

**为什么改**：`.map(f).flatten()` 是 `.and_then(f)` 的展开形式。`and_then` 是 `Option` 的标准组合子，语义更明确（"如果有值，尝试转换，失败则返回 None"），也避免了多余的中间 `Option<Option<T>>` 层。Clippy 规则 `map_flatten` 检测此模式。

---

#### 1.5 `src/api/cli.rs` — `needless_borrows_for_generic_args`（测试代码）

**改了什么**：

```rust
// Before:
let cli = Cli::try_parse_from(&["pi-wasm", "init"]).unwrap();

// After:
let cli = Cli::try_parse_from(["pi-wasm", "init"]).unwrap();
```

**为什么改**：`try_parse_from` 接受 `IntoIterator`，数组字面量 `["pi-wasm", "init"]` 本身就实现了该 trait，无需取引用 `&[...]`。多余的 `&` 增加认知负担且无运行时收益。Clippy 规则 `needless_borrows_for_generic_args` 检测此模式。

---

#### 1.6 `src/core/session/manager.rs` — `cast_abs_to_unsigned`

**改了什么**：

```rust
// Before:
let nsecs = ((ms % 1000).abs() as u32) * 1_000_000;

// After:
let nsecs = (ms % 1000).unsigned_abs() as u32 * 1_000_000;
```

**为什么改**：`i64::abs()` 返回 `i64`，当输入为 `i64::MIN` 时会溢出 panic（因为 `i64::MIN.abs()` 超出 `i64` 范围）。`unsigned_abs()` 返回 `u64`，保证不溢出。虽然 `ms % 1000` 不可能触发 `i64::MIN`，但 Clippy 规则 `cast_abs_to_unsigned` 推荐使用更安全的 API，这是防御性编程的最佳实践。

---

#### 1.7 `src/core/session/manager.rs` — `redundant_closure`

**改了什么**：

```rust
// Before:
let dt = chrono::DateTime::from_timestamp(secs, nsecs).unwrap_or_else(|| Utc::now());

// After:
let dt = chrono::DateTime::from_timestamp(secs, nsecs).unwrap_or_else(Utc::now);
```

**为什么改**：`|| Utc::now()` 是对 `Utc::now` 的冗余包装——闭包只是转发调用，无捕获、无额外逻辑。直接传递函数指针 `Utc::now` 语义等价且更简洁。Clippy 规则 `redundant_closure` 检测此模式。

---

#### 1.8 `src/ext/instance_wasmedge.rs` — `type_complexity`

**改了什么**：引入类型别名简化复杂类型签名。

```rust
// Before:（直接在字段上写完整类型）
host_invoke: Option<Arc<dyn Fn(&str) -> Result<String, AppError> + Send + Sync>>,

// After:（提取类型别名）
type HostInvokeFn = dyn Fn(&str) -> Result<String, AppError> + Send + Sync;
// ...
host_invoke: Option<Arc<HostInvokeFn>>,
```

**为什么改**：原始类型签名过长（`Option<Arc<dyn Fn(&str) -> Result<String, AppError> + Send + Sync>>`），在结构体字段中直接使用降低可读性。Clippy 规则 `type_complexity` 建议将复杂类型提取为别名。改后 `HostInvokeFn` 的语义（"宿主调用回调函数"）一目了然。

---

#### 1.9 `src/ext/instance_wasmedge.rs` — `unnecessary_to_owned`

**改了什么**：

```rust
// Before:
let _ = memory.set_data(resp_bytes.to_vec(), buf_ptr);

// After:
memory.set_data(resp_bytes, buf_ptr)...;
```

**为什么改**：`memory.set_data` 接受 `impl AsRef<[u8]>`，`resp_bytes`（`&[u8]`）已经满足。`.to_vec()` 会分配一块新内存拷贝数据，完全是浪费。Clippy 规则 `unnecessary_to_owned` 检测不必要的堆分配。

---

#### 1.10 `src/infra/audit.rs` — `default_constructed_unit_structs`

**改了什么**：

```rust
// Before:
let r = TracingAuditRecorder::default();

// After:
let r = TracingAuditRecorder;
```

**为什么改**：`TracingAuditRecorder` 是 unit struct（零字段），`::default()` 的实现和直接构造完全等价，但 `::default()` 看起来像是在做某种初始化。直接使用 `TracingAuditRecorder` 字面量更清晰地表达"这是一个无状态的结构体"。Clippy 规则 `default_constructed_unit_structs` 检测此模式。

---

#### 1.11 `src/ext/mod.rs` — `dead_code` (engine_stub, instance_stub)

**改了什么**：

```rust
#[allow(dead_code)]
mod engine_stub;
#[allow(dead_code)]
mod instance_stub;
```

**为什么改**：`engine_stub` 和 `instance_stub` 是 WasmEdge 不可用时的降级 stub 实现（mock 替代品）。当前默认构建包含真实 WasmEdge，stub 模块未被使用，但它们是架构上有意保留的（用于无 WasmEdge 环境的编译与测试）。加 `#[allow(dead_code)]` 在模块声明级别，避免内部每个函数都报警告。

---

### 2. RwLock 防毒化迁移

**涉及文件**：`src/core/tools.rs`、`src/infra/event_bus.rs`、`src/ext/plugin.rs`、`Cargo.toml`

**改了什么**：

```rust
// Before (3 个文件):
use std::sync::RwLock;
self.tools.write().unwrap().insert(key, t);
self.tools.read().unwrap().get(name)...

// After:
use parking_lot::RwLock;
self.tools.write().insert(key, t);
self.tools.read().get(name)...
```

`Cargo.toml` 新增依赖 `parking_lot = "0.12"`。

**为什么改**：`std::sync::RwLock` 有"锁中毒"（poison）机制——如果持锁线程 panic，锁会被标记为 poisoned，后续所有 `.read()/.write()` 都返回 `Err`。代码中用 `.unwrap()` 处理，意味着**一个线程的 panic 会级联导致所有后续访问 panic**。这在生产环境中是不可接受的连锁故障模式。

`parking_lot::RwLock` **没有 poison 机制**：持锁线程 panic 后，锁会正常释放，其他线程可以继续工作。这是 Rust 社区的主流选择（tokio、servo 等项目均使用 parking_lot）。额外收益：

- `.read()` / `.write()` 直接返回 guard，无需 `.unwrap()`，代码更简洁
- parking_lot 的性能在大多数场景下优于 std（更少的系统调用）

**为什么不用其他方案**：
- 保留 `std::sync::RwLock` + 用 `.unwrap_or_else(|e| e.into_inner())` 处理 poison：增加冗余代码，且语义不清晰
- 改用 `dashmap`：过重，当前场景不需要分段锁

---

### 3. Dispatcher 补齐 3 条缺失路由

**涉及文件**：`src/ext/dispatcher.rs`

**改了什么**：新增 `do_get_active_tools`、`do_set_active_tools`、`do_register_command` 三个方法，并在 `dispatch_async` 的 match 中添加对应路由。

**为什么改**：审查 `host-call-protocol.md` 和 `pi_bridge.js` 时发现，JS 侧 `pi.registerCommand()` 会发起 `tools.registerCommand` hostcall，`pi_bridge.js` 内部也引用了 `tools.getActiveTools` / `tools.setActiveTools`，但 Dispatcher 中没有这三条路由的实现——调用会落入 `_ => Err("unknown method")` 分支，导致静默失败。

这属于**协议实现不完整**——host-call-protocol.md 定义了 contract，`pi_bridge.js` 按 contract 发起调用，但宿主侧没有接住。补齐后确保协议两端对齐。

同时新增 4 个单元测试覆盖这些路由。

---

### 4. instance_wasmedge.rs 错误传播修复

**涉及文件**：`src/ext/instance_wasmedge.rs`

#### 4.1 `memory.set_data` 错误传播

**改了什么**：

```rust
// Before:
if out_len <= buf_cap {
    let _ = memory.set_data(resp_bytes, buf_ptr);
}

// After:
if out_len <= buf_cap {
    memory
        .set_data(resp_bytes, buf_ptr)
        .map_err(|_| CoreError::Execution(CoreExecutionError::HostFuncFailed))?;
}
```

**为什么改**：`let _ =` 显式丢弃了 `set_data` 的 `Result`——如果写入线性内存失败（比如 buf_ptr 越界），**错误会被静默吞掉**，JS 侧会拿到损坏的数据或旧数据。这违反了 Constitution 的错误传播原则："非预期错误不得静默吞没"。改后错误会通过 `?` 上报为 `CoreError::HostFuncFailed`，最终反映为 JS 侧的 hostcall 失败。

#### 4.2 `vm.run_func` 执行结果处理

**改了什么**：

```rust
// Before:
let _ = vm.run_func(Some("quickjs"), "_start", []);
Ok(serde_json::Value::Null)

// After:
match vm.run_func(Some("quickjs"), "_start", []) {
    Ok(_) => Ok(serde_json::Value::Null),
    Err(e) => {
        let msg = e.to_string();
        if msg.contains("exit code 0") || msg.contains("success") {
            Ok(serde_json::Value::Null)
        } else {
            Err(AppError::QuickJS(format!("script execution failed: {}", msg)))
        }
    }
}
```

**为什么改**：同样，`let _ =` 丢弃了 `run_func` 的结果。如果 JS 脚本执行失败（语法错误、运行时异常等），调用者**完全无法感知**，会以为脚本成功执行了。

改后区分两种情况：
- **"exit code 0" / "success"**：QuickJS `_start` 正常退出时，WasmEdge 有时也返回 `Err`（这是 WasmEdge 的 API 特性），但实际上脚本已成功执行，这种情况视为成功
- **其他错误**：真正的执行失败，上报为 `AppError::QuickJS`

---

## 二、P1 质量提升

### 5. 事件回调 TODO 注释

**涉及文件**：`src/ext/dispatcher.rs`（`do_events` 方法中 `"on"` 分支）

**改了什么**：为 `Box::new(|_| Ok(()))` 占位回调添加详细注释。

```rust
// 宿主侧注册占位回调；实际 JS 回调由 __pi_dispatch_event 触发 pi_bridge.js 中的 __pi_hooks。
// TODO: 长生命周期 VM 就绪后，此处应注入真实回调（通过 WasmInstance 回调到插件 JS）。
let id = self.event_bus.on(event_name, Box::new(|_| Ok(())));
```

**为什么改**：当前架构中事件注册到宿主 EventBus 时使用空回调，真正的 JS 侧回调依赖 `__pi_dispatch_event` 从另一条路径触发。这个设计意图不显而易见——看代码会疑惑"为什么注册了一个什么都不做的回调？"。注释说明了：

1. 这是设计上的**双层架构**（宿主注册 + JS 侧 hooks 实际执行）
2. 这是**临时方案**，待长生命周期 VM 就绪后需要改进
3. 改进方向是什么（注入真实回调到 WasmInstance）

不直接修改是因为这涉及架构级变更（需要 WasmInstance 支持长生命周期），不适合在本次整改中做。

---

### 6. JSONL 解析错误日志

**涉及文件**：`src/core/session/transcript.rs`

**改了什么**：

```rust
// Before:
Err(_) => continue, // 忽略无法解析的行

// After:
Err(e) => {
    warn!(line = trimmed, error = %e, "skipping unparseable JSONL entry");
    continue;
}
```

**为什么改**：JSONL transcript 是核心数据，解析失败意味着数据损坏或格式不兼容。静默 `continue` 会导致：

1. **数据丢失不可见**：用户不知道有条目被跳过了
2. **调试困难**：出现会话数据缺失时无法追踪原因

加上 `tracing::warn!` 后，错误会出现在日志中（包含原始行内容和解析错误详情），方便排查。选择 `warn` 而非 `error` 是因为单行解析失败不影响整体功能（其他行仍可正常读取）。

---

### 7. `effective_model` 支持 `default_model` fallback

**涉及文件**：`src/core/llm/openai.rs`

**改了什么**：

```rust
// Before:
fn effective_model(&self, request: &ChatRequest) -> String {
    request.model_override
        .as_deref()
        .unwrap_or(&request.model)
        .to_string()
}

// After:
fn effective_model(&self, request: &ChatRequest) -> String {
    request.model_override
        .as_deref()
        .unwrap_or(if request.model.is_empty() {
            &self.default_model
        } else {
            &request.model
        })
        .to_string()
}
```

**为什么改**：`OpenAiProvider` 有一个 `default_model` 字段（从 `LlmConfig` 读取），但 `effective_model` 从未使用它——即使 `request.model` 为空，也直接返回空字符串，导致 API 请求失败。

改后的优先级链为：`model_override` > `request.model`（非空时）> `self.default_model`。这同时消除了 `default_model` 的 `dead_code` 警告，且让配置文件中的 `default_model` 真正生效。

**`stream_timeout_sec`** 字段保留 `#[allow(dead_code)]`，因为它需要 `tokio::time::timeout` 集成才能使用，属于未来特性，不是简单的逻辑缺失。添加了 `TODO` 注释标明实现方向。

---

### 8. `pi_bridge.js` 补齐 `pi.off` / `pi.emit`

**涉及文件**：`assets/js/pi_bridge.js`

**改了什么**：在 `globalThis.pi` 对象中新增 `off` 和 `emit` 两个函数。

```javascript
off: function (eventName, listenerId) {
  return hostCall('events', 'off', { eventName: eventName, listenerId: listenerId });
},
emit: function (eventName, payload) {
  return hostCall('events', 'emit', { eventName: eventName, payload: payload || {} });
},
```

**为什么改**：审查 `host-call-protocol.md` 发现 events 模块定义了 `on`、`once`、`off`、`emit` 四个方法。`pi_bridge.js` 中 `pi.on` 和 `pi.once` 已实现，但 `pi.off`（取消监听）和 `pi.emit`（主动触发事件）缺失。Dispatcher 侧的 `do_events` 方法已经处理了 `"off"` 和 `"emit"` 路由，只是 JS 侧没有暴露入口。

缺失 `pi.off` 意味着插件**无法取消已注册的监听器**，可能导致内存泄漏（监听器永远不会被清理）。缺失 `pi.emit` 意味着插件**无法主动触发事件**，限制了插件间通信能力。

---

### 9. 文档修正

#### 9.1 `agents/wasm_plugin_agent.md` — 全局对象名称

**改了什么**：将"全局 `agent` 对象"改为"全局 `pi` 对象（`globalThis.pi`）"。

**为什么改**：实际代码中 `pi_bridge.js` 构建的是 `globalThis.pi`，不是 `globalThis.agent`。文档与实现不一致会误导开发者。

#### 9.2 `openspec/changes/001-mvp/design.md` — 失效链接

**改了什么**：将引用 `archive/pi-ecosystem-alignment-check.md`（已不存在的文件）替换为说明文字，指向 `Architecture.md` 和 `host-call-protocol.md`。

**为什么改**：原文件已归档移除，链接变成死链。点击后 404 会让开发者困惑。改为说明"已归档移除"，并指引到当前有效的替代参考文档。

---

### 10. `dead_code` 审查：`src/infra/platform.rs`

**改了什么**：保持不变（`#[allow(dead_code)]` 保留）。

**为什么这样决定**：`current_dir()`、`SystemInfo`、`system_info()` 这三个函数/类型虽然当前未被调用，但它们是为 `pi_wasm doctor` 子命令预留的（doctor 需要检测系统环境信息）。删除它们会在 doctor 实现时重新编写，且这些函数已有完整的单元测试覆盖。属于**有意预留**的代码。

---

### 11. README.md

**涉及文件**：新增 `README.md`

**为什么新增**：项目根目录没有 README，这是项目规范的基本要求。新开发者或审查者打开仓库时没有入口文档引导。README 包含：快速开始（WasmEdge 安装 + 构建 + 测试）、项目结构、架构概览、规范文档引用。

---

## 三、Cargo.toml 依赖变更

**改了什么**：新增 `parking_lot = "0.12"`。

**为什么选 0.12**：这是 parking_lot 的最新稳定版本。parking_lot 是 Rust 生态中最成熟的替代锁库（每月数百万下载），API 与 std 高度一致（只需改 use 声明和去掉 unwrap），迁移成本最低。

---

## 四、数据汇总

| 类别 | 改动数 | 文件数 |
|:---|:---|:---|
| Clippy 修复 | 11 种警告类型，19 处 | 8 个文件 |
| RwLock 迁移 | ~15 处 unwrap 消除 | 4 个文件 |
| 缺失路由补齐 | 3 条路由 + 4 个测试 | 1 个文件 |
| 错误传播修复 | 2 处静默吞错 | 1 个文件 |
| JS 桥接补齐 | 2 个函数 | 1 个文件 |
| 文档修正 | 3 处 | 3 个文件 |
| 新增文件 | README.md | 1 个文件 |
| **合计** | — | **15 个文件** |
