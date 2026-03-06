这份《集成测试规范》旨在为开发团队提供一套标准化的测试流程和准则。Rust 的集成测试主要关注**外部接口（Public API）**、**模块间交互**以及**外部依赖（数据库、网络等）**。

---

# Rust 程序集成测试规范 (v1.0)

## 1. 测试目标
*   所有测试均需要编写独立的测试代码，不是复用单元测试代码
*   验证多个模块组合后的行为是否符合预期。
*   确保从外部（作为 Crate 使用者）调用公共接口时的正确性。
*   验证程序与外部系统（如数据库、文件系统、第三方 API）的集成。
*   **集成测试不得脱离真实环境**：验证与外部系统（第三方 API、数据库、文件系统等）的协作时，必须使用真实依赖而非 Mock；外部协作与模块间协作均属“协作”，均需在真实环境下验证。

## 2. 目录结构与组织
Rust 默认将集成测试放在项目根目录的 `tests/` 文件夹下。

### 2.1 基础布局
```text
my_project/
├── Cargo.toml
├── src/
│   └── lib.rs       # 只有库(lib)项目才能进行标准的集成测试
└── tests/
    ├── common/      # 共享工具模块
    │   └── mod.rs   # 存放初始化、Mock 数据等逻辑
    ├── api_tests.rs # 具体的集成测试文件
    ├── db_tests.rs  # 按功能维度划分
    └── llm_tests.rs # LLM 与真实外部 API 协作（必选维度）
```

### 2.2 规范要求
*   **黑盒测试**：集成测试文件应视为 Crate 的外部使用者，只能访问 `pub` 修饰的内容。
*   **避免重复编译**：如果 `tests/` 下文件过多，建议通过子模块形式组织，减少生成的二进制文件数量（每个 `.rs` 文件都会编译成独立的二进制）。
*   **共享代码**：公共配置或辅助函数必须放在 `tests/common/mod.rs` 中，以防止被 Rust 识别为独立的测试入口（避免出现 "dead code" 警告）。

## 3. 命名规范
*   **文件命名**：采用小写字母加下划线，反映测试的主体，如 `auth_flow.rs`。
*   **函数命名**：遵循 `test_[功能]_[场景]_[预期结果]` 格式。
    *   示例：`test_user_login_with_invalid_password_should_fail()`
*   **断言描述**：在使用 `assert!` 等宏时，建议添加自定义错误信息。

## 4. 测试编写标准

### 4.1 测试模式：AAA (Arrange, Act, Assert)
每个测试函数应包含明显的三个阶段：
1.  **Arrange (准备)**：初始化数据、配置环境、Mock 外部服务。
2.  **Act (执行)**：调用被测的公共接口。
3.  **Assert (断言)**：验证输出结果、状态变更或副作用。

### 4.2 错误处理
*   **使用 Result**：推荐测试函数返回 `Result<(), Box<dyn Error>>`，以便在测试中使用 `?` 操作符处理错误，而不是到处用 `unwrap()`。
    ```rust
    #[tokio::test]
    async fn test_create_user() -> Result<(), Box<dyn std::error::Error>> {
        let client = common::setup_client()?;
        let res = client.post("/user").send().await?;
        assert_eq!(res.status(), 201);
        Ok(())
    }
    ```

### 4.3 异步测试
*   使用 `#[tokio::test]` 或 `#[async_std::test]`。
*   确保测试间不共享会导致竞争的状态（如共用同一个数据库表）。

## 5. 外部依赖处理

### 5.1 数据库集成
*   **隔离性**：每个测试用例应在独立的事务中运行并回滚，或者使用唯一的 ID/前缀隔离数据。
*   **工具**：推荐使用 `sqlx` 的测试宏或 `testcontainers-rs` 启动临时容器。

### 5.2 Mocking 策略
*   **仅单元测试与内部模块未完成建设时**可采用 wiremock、mockito、mockall 等模拟外部服务。
*   **集成测试**：涉及外部 API（如 LLM 服务）时，应使用真实端点；通过环境变量（如 `OPENAI_API_KEY`）控制可用性；**无 key 或不可达时用例须失败，不得 `#[ignore]` 或跳过**；运行全量集成测试前须配置好所需环境（如 `OPENAI_API_KEY`）。
*   **代码级 Mock**：单元测试中对于难以集成的逻辑，可使用 `mockall` 宏生成 Mock 对象；集成测试中不 Mock 外部服务。

### 5.3 真实环境要求
*   集成测试以真实环境为默认：与外部系统（数据库、第三方 API、文件系统等）的协作必须在真实环境下验证。
*   Mock 仅用于单元测试或尚未完成建设的内部模块；集成测试套件中必须包含与真实外部依赖协作的用例（如 LLM 的 `llm_tests.rs`）；（无 key 或不可达时要求见 5.2）。

## 6. 断言与工具库
推荐集成以下工具以增强测试表达力：
*   **`pretty_assertions`**：在断言失败时提供更易读的 Diff。
*   **`rstest`**：支持参数化测试（类似于 Pytest 的 parametrize）。
*   **`claims`**：专门用于对 `Result` 和 `Option` 进行优雅断言。

## 7. 执行与持续集成 (CI)

### 7.1 本地执行
*   运行所有集成测试：`cargo test --test '*'`
*   运行特定文件：`cargo test --test api_tests`
*   显示打印输出：`cargo test -- --nocapture`

### 7.2 串行执行
对于涉及全局资源（如固定端口、单例数据库）的测试，需强制单线程执行：
`cargo test -- --test-threads=1`

### 7.3 CI 检查项
在流水线（如 GitHub Actions）中，集成测试应包含：
1.  **代码风格检查**：`cargo fmt --check`
2.  **静态分析**：`cargo clippy`
3.  **漏洞扫描**：`cargo audit`
4.  **覆盖率要求**：集成测试应覆盖核心业务路径（使用 `cargo-tarpaulin` 统计）。

### 7.4 全量集成测试
跑全量集成测试时与外部真实环境交互

## 8. 最佳实践建议
*   **不要过度 Mock**：集成测试的价值在于真实性，如果所有外部依赖都被 Mock 了，那它就变成了单元测试。
*   **环境一致性**：确保开发环境、测试环境和 CI 环境的基础设施版本（如 PostgreSQL 版本）完全一致。
*   **测试清理**：利用 `Drop` trait 或特定的清理脚本，确保测试产生的临时文件或数据被及时删除。
*   **注释**：对于复杂的集成逻辑，必须在测试函数头部注释说明该测试的业务场景。



## 9. 日志与链路追踪规范

### 9.0 强制要求（集成测试门禁）

为保证失败可定位、行为可追溯，**每个集成测试用例必须同时满足**：

1. **初始化**：用例入口调用 `common::setup_logging()`（或共享入口调用一次），避免重复 init（使用 `Once`）。
2. **上下文**：为每个测试用例创建 `info_span!`（或使用 `#[instrument]`）标注用例名与关键参数（如 plugin_id、session_key）。
3. **AAA 日志锚点**：在 Arrange / Act / Assert 三个阶段的关键步骤**至少各记录一条** `tracing::info!`（必要时补 `tracing::debug!` 记录关键变量）。

> 说明：默认 `cargo test` 会捕获输出，需使用 `-- --nocapture` 才能在终端实时看到日志（见 9.4）。

### 9.1 栈技术选型
*   **日志门面**：统一使用 `tracing` crate 代替 `log`。`tracing` 支持异步、结构化日志，并能完美兼容 `log` 库。
*   **初始化器**：使用 `tracing-subscriber` 处理日志的输出与过滤。

### 9.2 初始化策略
集成测试的文件是独立的二进制文件，因此每个测试入口（或共享的初始化函数）必须显式初始化订阅器。

*   **避免重复初始化**：由于多个测试用例可能并行执行，多次调用初始化函数会导致 `panic`。应使用 `std::sync::Once` 确保初始化逻辑只运行一次。
*   **公共初始化函数**（在 `tests/common/mod.rs` 中）：
    ```rust
    use std::sync::Once;
    use tracing_subscriber::{fmt, EnvFilter, prelude::*};

    static INIT: Once = Once::new();

    pub fn setup_logging() {
        INIT.call_once(|| {
            tracing_subscriber::registry()
                .with(fmt::layer().with_test_writer()) // 关键：使用 test_writer 让 cargo test 捕获输出
                .with(EnvFilter::from_default_env().add_directive(tracing::Level::DEBUG.into()))
                .init();
        });
    }
    ```

### 9.3 结构化日志实践
*   **上下文关联**：在测试中使用 `info_span!` 或 `#[instrument]` 宏记录当前测试的上下文（如用户 ID、请求 ID）。
*   **关键节点记录**：在测试的 AAA 阶段记录关键转折点。
    ```rust
    #[tokio::test]
    async fn test_order_lifecycle() {
        setup_logging();
        let _span = tracing::info_span!("test_order", order_id = "123").entered();

        tracing::info!("Starting order placement...");
        // ... 执行逻辑
        tracing::debug!("Internal state checked.");
    }
    ```

### 9.4 日志查看控制
*   **静默模式（默认）**：直接运行 `cargo test` 时，日志不会打印，除非断言失败；集成测试的 tracing 输出需配合本节的 `--nocapture` 才能实时查看。
*   **实时查看**：若需查看运行中的日志，使用 `--nocapture` 参数；**集成测试**建议：`cargo test --test '*' -- --nocapture`（可按需加 `RUST_LOG=debug`），避免误以为未打日志。
    ```bash
    cargo test -- --nocapture
    cargo test --test '*' -- --nocapture
    ```
*   **级别控制**：利用 `RUST_LOG` 环境变量动态调整日志等级：
    ```bash
    RUST_LOG=debug cargo test --test api_tests -- --nocapture
    ```

### 9.5 断言日志输出 (可选)
如果业务逻辑要求必须产生特定的日志（如审计日志），应使用 `tracing-test` crate：
```rust
#[tokio::test]
#[tracing_test::traced_test]
async fn test_security_alert_logged() {
    trigger_security_event().await;
    // 验证是否输出了包含特定内容的日志
    assert!(logs_contain("Security threat detected"));
}
```

### 9.6 CI 环境下的日志保存
*   **强制着色**：在 CI 配置中设置 `FORCE_COLOR=1`，确保下载下来的日志文件带有颜色标记，方便阅读。
*   **文件归档**：对于极其复杂的集成测试，建议将日志重定向到文件，并在 CI 失败时上传这些文件作为 Artifacts。

---

### 更新后的目录结构参考：
```text
tests/
├── common/
│   ├── mod.rs      # 包含 setup_logging() 和 Once 逻辑
│   └── helpers.rs
├── api_tests.rs    # 开头调用 common::setup_logging()
├── data_tests.rs
└── llm_tests.rs    # LLM 与真实 API 协作（无 key 时要求见 5.2）
```

### 核心原则：
1.  **可观测性**：测试失败时，日志必须能清晰展现从请求进入到报错的全链路流程。
2.  **整洁性**：正常通过的测试不应在终端输出大量无用日志（利用 `with_test_writer` 实现）。
3.  **结构化**：优先使用 `field=value` 的形式记录关键变量，而非拼凑字符串。

----

# 理论与实践结合

> 供 Agent 参考，不强制约束。完整场景示例、三不原则、审计/Teardown/DoD 及总结表见 **[集成测试实践参考](INTEGRATION_TEST_PRACTICE.md)**。

