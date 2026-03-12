# 集成测试日志与链路追踪规范

> 本文档为《集成测试规范》**第 9 章**子文档；主文档见 [INTEGRATION_TEST_SPEC.md](INTEGRATION_TEST_SPEC.md)。规定集成测试中日志与链路追踪的技术选型、初始化、结构化实践及查看控制。

---

## 1. 栈技术选型

*   **日志门面**：统一使用 `tracing` crate 代替 `log`。`tracing` 支持异步、结构化日志，并能完美兼容 `log` 库。
*   **初始化器**：使用 `tracing-subscriber` 处理日志的输出与过滤。

## 2. 初始化策略

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

## 3. 结构化日志实践

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

## 4. 日志查看控制

*   **静默模式（默认）**：直接运行 `cargo test` 时，日志不会打印，除非断言失败；集成测试的 tracing 输出需配合本节的 `--nocapture` 才能实时查看。
*   **实时查看**：若需查看运行中的日志，使用 `--nocapture` 参数；**集成测试**建议：`RUST_LOG=pi_wasm=debug,info cargo test --test '*' -- --nocapture`，避免误以为未打日志。
*   **按顺序输出（便于阅读）**：默认多线程并行跑测试时，各用例的 Arrange/Act/Assert 日志会交错。若希望**同一测试的日志连续、按执行顺序**出现，请加 `--test-threads=1` 串行执行。
    ```bash
    RUST_LOG=pi_wasm=debug,info cargo test -- --nocapture
    RUST_LOG=pi_wasm=debug,info cargo test --test '*' -- --nocapture
    # 日志按测试顺序、不交错（推荐排查时使用）：
    RUST_LOG=pi_wasm=debug,info cargo test --test '*' -- --nocapture --test-threads=1
    ```
*   **级别控制**：利用 `RUST_LOG` 环境变量动态调整日志等级：
    ```bash
    RUST_LOG=pi_wasm=debug,info cargo test --test api_tests -- --nocapture
    ```

## 5. 断言日志输出 (可选)

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

## 6. CI 环境下的日志保存

*   **强制着色**：在 CI 配置中设置 `FORCE_COLOR=1`，确保下载下来的日志文件带有颜色标记，方便阅读。
*   **文件归档**：对于极其复杂的集成测试，建议将日志重定向到文件，并在 CI 失败时上传这些文件作为 Artifacts。

---

## 更新后的目录结构参考

```text
tests/
├── common/
│   ├── mod.rs      # 包含 setup_logging() 和 Once 逻辑
│   └── helpers.rs
├── api_tests.rs    # 开头调用 common::setup_logging()
├── data_tests.rs
└── llm_tests.rs    # LLM 与真实 API 协作（无 key 时要求见 5.2）
```

## 核心原则

1.  **可观测性**：测试失败时，日志必须能清晰展现从请求进入到报错的全链路流程。
2.  **整洁性**：正常通过的测试不应在终端输出大量无用日志（利用 `with_test_writer` 实现）。
3.  **结构化**：优先使用 `field=value` 的形式记录关键变量，而非拼凑字符串。
