# Rust 惯用写法与 Clippy 规则速查 (Rust Idioms Spec)

本规范从项目代码审查（2026-03-09）中提炼，收录常见的 Clippy 警告模式及其修复方式。每条规则附带 lint 名称、原理说明、Before/After 对照代码。

**门禁要求**：`cargo clippy --all-targets` 必须零警告。新代码提交前须通过此检查。

---

## I-1. Option/Result 组合子：使用最精简的表达

### 涉及 lint

- `clippy::map_flatten`
- `clippy::unnecessary_map_or`

### 原理

`.map(f).flatten()` 是 `.and_then(f)` 的展开形式，多了一层中间 `Option<Option<T>>`，增加认知负担。`.map_or(false, |x| x == v)` 等价于 `== Some(v)`，后者更直观。Clippy 检测这些可简化的模式，推荐使用 Rust 标准组合子。

### 实践

```rust
// BAD — map + flatten 冗余
let path = config_path
    .map(|s| normalize_path(s).ok())
    .flatten();

// GOOD — and_then 语义更明确
let path = config_path
    .and_then(|s| normalize_path(s).ok());
```

```rust
// BAD — map_or 判等
if entry_id(&entry).map_or(false, |s| s == id) { ... }

// GOOD — 直接比较 Option
if entry_id(&entry) == Some(id) { ... }
```

---

## I-2. 多余借用：泛型参数已满足时不加 `&`

### 涉及 lint

- `clippy::needless_borrows_for_generic_args`

### 原理

当函数接受 `impl IntoIterator` 等泛型参数时，数组字面量 `["a", "b"]` 已满足 trait，无需取引用 `&["a", "b"]`。多余的 `&` 增加认知负担且无运行时收益。

### 实践

```rust
// BAD — 多余的 &
let cli = Cli::try_parse_from(&["tomcat", "init"]).unwrap();

// GOOD — 数组字面量直接满足 IntoIterator
let cli = Cli::try_parse_from(["tomcat", "init"]).unwrap();
```

---

## I-3. 数值安全：使用 `unsigned_abs()` 替代 `.abs() as`

### 涉及 lint

- `clippy::cast_abs_to_unsigned`

### 原理

`i64::abs()` 返回 `i64`，当输入为 `i64::MIN` 时会溢出 panic（`i64::MIN.abs()` 超出 `i64` 范围）。`unsigned_abs()` 返回 `u64`，保证不溢出。即使当前值域不可能触发 `i64::MIN`（如 `ms % 1000`），也应使用更安全的 API——这是防御性编程。

### 实践

```rust
// BAD — abs() 有溢出风险
let nsecs = ((ms % 1000).abs() as u32) * 1_000_000;

// GOOD — unsigned_abs() 保证无溢出
let nsecs = (ms % 1000).unsigned_abs() as u32 * 1_000_000;
```

---

## I-4. 冗余闭包：无捕获、仅转发时直接传函数指针

### 涉及 lint

- `clippy::redundant_closure`

### 原理

`|| f()` 在闭包无捕获且仅转发调用时，与直接传递函数指针 `f` 语义等价。函数指针更简洁，且编译器可更好地内联优化。

### 实践

```rust
// BAD — 冗余包装
let dt = DateTime::from_timestamp(secs, nsecs)
    .unwrap_or_else(|| Utc::now());

// GOOD — 直接传递函数指针
let dt = DateTime::from_timestamp(secs, nsecs)
    .unwrap_or_else(Utc::now);
```

---

## I-5. 类型复杂度：超长类型签名提取为 `type` 别名

### 涉及 lint

- `clippy::type_complexity`

### 原理

超过约 60 字符的类型签名（如 `Option<Arc<dyn Fn(&str) -> Result<String, AppError> + Send + Sync>>`）在结构体字段或函数参数中直接使用会严重降低可读性。提取为 `type` 别名后，名称本身就传达了语义。

### 实践

```rust
// BAD — 完整类型签名难以阅读
pub struct WasmInstance {
    host_invoke: Option<Arc<dyn Fn(&str) -> Result<String, AppError> + Send + Sync>>,
}

// GOOD — 类型别名传达语义
type HostInvokeFn = dyn Fn(&str) -> Result<String, AppError> + Send + Sync;

pub struct WasmInstance {
    host_invoke: Option<Arc<HostInvokeFn>>,
}
```

---

## I-6. 避免无意义堆分配

### 涉及 lint

- `clippy::unnecessary_to_owned`

### 原理

当函数接受 `impl AsRef<[u8]>` 时，`&[u8]` 已满足。调用 `.to_vec()` 会分配一块新内存拷贝数据，完全是浪费。在 Wasm 线性内存操作等高频路径上，不必要的堆分配会影响性能。

### 实践

```rust
// BAD — to_vec() 产生不必要的堆分配
memory.set_data(resp_bytes.to_vec(), buf_ptr)?;

// GOOD — &[u8] 直接满足 AsRef<[u8]>
memory.set_data(resp_bytes, buf_ptr)?;
```

---

## I-7. Unit struct 构造：不使用 `::default()`

### 涉及 lint

- `clippy::default_constructed_unit_structs`

### 原理

Unit struct（零字段结构体）的 `::default()` 实现与直接构造完全等价，但 `::default()` 看起来像在做某种初始化工作，产生误导。直接使用字面量更清晰地表达"这是一个无状态标记类型"。

### 实践

```rust
// BAD — 看起来有初始化逻辑
let recorder = TracingAuditRecorder::default();

// GOOD — 直接构造，意图明确
let recorder = TracingAuditRecorder;
```

---

## I-8. Doc comment 格式：`///` 与 item 之间禁止空行

### 涉及 lint

- `clippy::empty_line_after_doc_comments`

### 原理

Rust 的 `///` doc comment 语义是附着到紧随其后的 item。如果 `///` 与 item 之间存在空行，第一段注释会变成"悬空"（不附着到任何 item），在 `cargo doc` 中不会出现，且可能被误读为两段不相关的注释。

### 实践

```rust
// BAD — 空行导致第一段 doc comment 悬空
/// 使用 [`LogConfig`] 初始化 tracing。
///
/// 优先使用环境变量 `RUST_LOG`。

pub fn init_logging(cfg: &LogConfig, log_dir: Option<&Path>) -> Result<(), AppError> { ... }

// GOOD — doc comment 紧贴 item
/// 使用 [`LogConfig`] 初始化 tracing。
///
/// 优先使用环境变量 `RUST_LOG`。
pub fn init_logging(cfg: &LogConfig, log_dir: Option<&Path>) -> Result<(), AppError> { ... }
```

---

## 验收标准

- `cargo clippy --all-targets` 零警告
- 新代码遵循上述 8 条惯用法规则
- Code Review 时可引用本文档规则编号（如 "违反 I-3"）进行标注
