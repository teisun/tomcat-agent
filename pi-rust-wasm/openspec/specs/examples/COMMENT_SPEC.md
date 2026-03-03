
---

# Rust 代码注释与文档规范 (Rust Comment Spec)

## 1. 总体原则
- **注释是为了解释“为什么”，而不是“是什么”**：代码本身应尽可能自解释。
- **保持同步**：修改代码逻辑时，必须同步更新相关文档注释。
- **惯用法优先**：遵循 Rust 官方标准库的文档风格（Rustdoc）。
- **禁止废话**：不要写 `// 这是一个设置名称的方法` 这种显而易见的注释。

---

## 2. 文档注释 (Doc Comments)

### 2.1 模块级注释 (`//!`)
- **位置**：位于文件最顶部。
- **用途**：描述当前模块的职责、与其他模块的关系、以及整体架构设计。
- **示例**：
  ```rust
  //! # 事件总线模块 (Event Bus)
  //!
  //! 该模块实现了基于订阅者模式的异步事件分发系统。
  //! 它支持跨插件的通信，并保证了单监听器崩溃不会影响全局流程。
  ```

### 2.2 项级注释 (`///`)
- **位置**：`struct`、`enum`、`fn`、`trait` 定义上方。
- **格式**：首句为简短摘要，后续段落详细展开。
- **标准小节**（按需添加）：
  - `# Examples`: 演示如何使用该函数（会被 `cargo test` 作为文档测试运行）。
  - `# Errors`: 如果返回 `Result`，描述哪些情况会导致错误。
  - `# Panics`: 如果函数可能触发 `panic!`，必须明确说明触发条件。
  - `# Safety`: 如果是 `unsafe` 函数，必须解释调用者需满足的内存安全前提。

---

## 3. 函数注释模版

对于核心 API，应严格遵守以下结构：

```rust
/// 执行指定的原子工具命令。
///
/// 该函数会检查权限白名单，并在独立的安全沙箱中启动子进程。
///
/// # Arguments
/// * `command` - 待执行的原始命令字符串
/// * `args` - 传递给命令的参数列表
///
/// # Returns
/// 返回执行结果的 `stdout`；如果执行超时或被拦截，返回 `AppError`。
///
/// # Errors
/// * `AppError::Permission` - 当命令不在 `PrimitiveConfig` 白名单中时返回。
/// * `AppError::Io` - 当底层系统无法启动进程时返回。
///
/// # Examples
/// ```
/// let result = primitive_tools::execute("ls", vec!["-la"]).unwrap();
/// assert!(result.contains("src"));
/// ```
pub fn execute(command: &str, args: Vec<&str>) -> Result<String, AppError> {
    // ...
}
```

---

## 4. 内部逻辑注释 (`//`)

用于解释复杂的实现细节，而不是 API 文档。

- **复杂逻辑说明**：在复杂的算法或状态机旁解释设计思路。
- **TODO/FIXME**：
  - `// TODO(username): 说明需要完成的功能`
  - `// FIXME: 说明存在的已知 Bug 或性能瓶颈`
- **代码块标记**：
  ```rust
  // --- 步骤 1: 权限校验 ---
  check_permission(ctx)?;

  // --- 步骤 2: 执行核心逻辑 ---
  // 这里使用原子操作是为了防止多线程下的状态竞争
  state.fetch_add(1, Ordering::SeqCst);
  ```

---

## 5. Rustdoc 特色技巧

- **跨链接**：使用 ``[`TypeName`]`` 自动生成跳转链接。
  - `/// 返回一个 [`AppConfig`] 实例。`
- **Markdown 强化**：在注释中使用表格、列表或 Mermaid 图表（如果配置了相关插件）。
- **隐藏代码行**：在 Example 中，不希望显示的设置代码前加 `#`。
  ```rust
  /// ```
  /// # let config = AppConfig::default(); // 这行在文档中不显示
  /// let client = LlmClient::new(config);
  /// ```
  ```

---

## 6. Cursor / AI 提示词指令

当你要求 Cursor 写代码时，可以附加以下要求：

> **Prompt Context:**
> "请遵循项目注释规范：
> 1. 公开接口必须包含 `///` 文档注释，并附带 `# Examples` 和 `# Errors`。
> 2. 复杂逻辑块需使用 `// --- 标题 ---` 进行分割。
> 3. 使用 `[`Type`]` 语法进行交叉引用。
> 4. 严禁使用裸 `unwrap()`，若必须使用，请在上方注释 `// SAFETY: 说明为什么这里一定不会 Panic`。"

---

## 7. 验收标准
- 运行 `cargo doc --open`，文档页面应清晰、无断链、示例可读。
- 关键模块的文档测试 (`cargo test --doc`) 必须全部通过。
- 复杂枚举变体必须有注释说明每个变体的用途。

```rust
pub enum AppError {
    /// IO 操作失败，通常是磁盘空间不足或权限问题
    Io(std::io::Error),
    /// 插件运行时错误，例如 Wasm 内存溢出
    Plugin(String),
    // ...
}
```