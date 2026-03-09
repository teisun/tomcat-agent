# 集成测试鲁棒性保障：异常场景与边界条件 (Robustness & Edge Cases)

> 本文档为《集成测试规范》**第 10 章**子文档；主文档见 [INTEGRATION_TEST_SPEC.md](INTEGRATION_TEST_SPEC.md)。规定异常与边界场景的覆盖要求及实践写法。

**本规范为集成测试门禁之一**：全量集成测试须包含并通过本文规定的异常与边界类用例（详见主文档第 10 节门禁与验收清单）。

---

## 1. 核心理论：从“逻辑正确”到“架构鲁棒”

单元测试关注的是代码的**正确性 (Correctness)**，而集成测试关注的是系统的**鲁棒性 (Robustness)**。在复杂的分布式或插件化系统中，模块间的“缝隙”往往是错误滋生的温床。

### 1.1 异常分类模型
集成测试必须覆盖以下三个维度的边界：

1.  **环境边界 (Environmental)**：
    *   **资源受限**：磁盘空间满、内存溢出、文件权限被锁定。
    *   **网络抖动**：目标 API（如 OpenAI）超时、连接被重置、DNS 解析失败。
2.  **契约边界 (Contractual)**：
    *   **协议违规**：外部插件返回了不符合 Schema 的 JSON、Header 缺失、编码错误。
    *   **版本不一致**：宿主引擎升级后，旧版插件的调用是否会导致崩溃。
3.  **状态边界 (State-based)**：
    *   **竞态条件**：多个插件同时修改同一个文件，锁机制是否生效。
    *   **非法时序**：在未登录/未初始化时调用核心 API，系统是否能优雅拦截。

---

## 2. 实践指南：如何编写异常集成测试

### 2.1 模拟外部失败 (Failure Injection)

> **与规范的协调**：本类测试属于 [INTEGRATION_TEST_SPEC.md 5.2](INTEGRATION_TEST_SPEC.md#52-mocking-策略) 允许的「异常/故障注入」例外，仅用于验证宿主在外部失败时的行为（重试、退避、熔断等），**不作为**替代真实 API 协作的用例；与真实端点协作用例并存。

**实践要求**：不要只测成功的 API 调用。使用 `wiremock` 或自定义 Mock Server 模拟外部服务的恶意行为。

*   **案例**：模拟 OpenAI 返回 429 (Rate Limit) 或 503 (Overloaded)。
```rust
#[tokio::test]
async fn test_llm_rate_limit_retry_strategy() {
    // 1. Arrange: 模拟一个每两次请求就报一次 429 的服务器
    let mock_server = MockServer::start().await;
    mock_server.register(
        ResponseTemplate::new(429).set_delay(Duration::from_millis(100))
    ).await;

    // 2. Act: 调用宿主引擎的 LLM 模块
    let result = engine.call_llm("Hello").await;

    // 3. Assert: 验证引擎是否触发了指数退避重试，而不是直接崩溃
    assert!(result.is_retry_attempted());
    assert_eq!(result.final_status(), "Success");
}
```

### 2.2 强制超时控制 (The Timeout Rule)
**实践要求**：所有的集成测试必须包裹在超时逻辑中。这既是测试，也是对系统“挂起”处理能力的验证。

*   **案例**：验证当插件执行死循环时，宿主能否强制强杀沙箱。
```rust
#[tokio::test]
async fn test_plugin_infinite_loop_termination() {
    let plugin_code = "while(true) {}"; // 恶意死循环
    
    // 使用 tokio::time::timeout 确保测试本身不挂起
    let result = tokio::time::timeout(
        Duration::from_secs(5), 
        engine.run_plugin(plugin_code)
    ).await;

    // 断言结果：应该是由于超时被宿主主动终止，而不是一直运行
    assert!(matches!(result, Ok(Err(EngineError::SandboxTerminated))));
}
```

### 2.3 脏数据与非法路径 (Security & Input)
**实践要求**：针对 4 原语等高危操作，必须测试“越权”场景。

*   **案例**：验证路径白名单的边界。
```rust
#[tokio::test]
async fn test_path_traversal_prevention() {
    let engine = Engine::init().with_root("/tmp/safe_zone");
    
    // 尝试使用相对路径绕过限制
    let illegal_path = "../../../etc/passwd";
    let result = engine.read_file(illegal_path).await;

    // 断言：必须返回 PermissionDenied，且审计日志中有拦截记录
    assert!(result.is_err());
    assert!(audit_log.has_entry("Security Violation", illegal_path));
}
```

### 2.4 资源泄露验证 (Long-run Stability)
**实践要求**：对于涉及 WasmEdge 实例等重资源的模块，模拟循环操作。

*   **案例**：循环加载卸载插件 50 次，检查内存增长。
```rust
#[test]
fn test_plugin_reload_no_memory_leak() {
    let initial_mem = get_process_memory();
    for _ in 0..50 {
        let p = engine.load_plugin("my_plugin.wasm");
        engine.unload_plugin(p);
    }
    let final_mem = get_process_memory();
    
    // 允许合理的波动，但不能有明显的线性增长
    assert!(final_mem - initial_mem < 5 * 1024 * 1024); // 小于 5MB
}
```

---

## 3. 异常测试的“断言”准则

在异常集成测试中，断言不应只看 `is_err()`，而应包含：
1.  **错误分类断言**：错误类型是否符合预期（是 `NetworkError` 还是 `AuthError`）。
2.  **副作用断言**：即使失败了，是否留下了垃圾文件？（应该被清理）。
3.  **日志断言**：错误是否被完整地记录到了 `tracing` 日志中，且包含了足够的上下文（如 Request ID）。

---

## 4. 总结：异常测试清单 (Checklist)

在编写每个功能的集成测试时，请问自己四个问题：
*   [ ] 如果依赖的服务挂了，我的模块会无限等待吗？
*   [ ] 如果输入的 JSON 缺了一个字段，我的系统会直接 Panic 吗？
*   [ ] 如果用户在操作一半时强制断开连接，我的数据会损坏吗？
*   [ ] 所有的异常路径在 `Audit Log` 中都有迹可循吗？

**结论：** 只有通过了这些“折磨”的集成测试，我们的 `pi-rust-wasm` 引擎才真正具备了生产级发布的资格。

---

## 5. 鲁棒性用例放置约定

为避免测试文件膨胀、每个模块不必单独再建「某某_robustness_tests.rs」，采用**两层分工**：

| 放置位置 | 适用场景 | 说明 |
|----------|----------|------|
| **各功能的 `*_tests.rs`**（如 `session_tests.rs`、`plugin_tests.rs`、`llm_tests.rs`、`primitives_tools_tests.rs`） | 该功能自身的异常与边界 | 主路径用例与**该功能**的鲁棒性/边界用例（错误类型、非法输入、权限拒绝、超时等）写在同一文件内。 |
| **`robustness_tests.rs`** | 跨模块或通用的鲁棒性 | 仅放无法归到单一功能域的用例，例如：契约/Schema 违规（多模块共用）、错误分类断言、重复加载/卸载导致的状态一致性等。 |

- **不要求**为每个模块单独再建「某某_robustness_tests.rs」。
- 门禁仍以全量 `cargo test --test '*'` 通过为准；鲁棒性覆盖由「各 `*_tests.rs` 内边界用例」+「`robustness_tests.rs` 中跨模块/通用用例」共同满足。
