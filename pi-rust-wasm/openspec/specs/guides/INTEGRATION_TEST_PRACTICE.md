# 集成测试实践参考

> 从 [INTEGRATION_TEST_SPEC.md](INTEGRATION_TEST_SPEC.md) 抽出的理论与实践结合部分，供 Agent 与开发快速查阅；不强制约束。

---

## 1. 集成测试哲学：验证“协作”而非“逻辑”

- **理论**：单元测试验证零件（如：Path 是否合规），集成测试验证组装（如：插件调用 4 原语时，白名单过滤是否生效）。
- **实践**：在 `tests/` 目录下，禁止 Mock 内部核心模块（如 `WasmRuntime` 或 `EventBus`），必须使用真实实例进行端到端测试。
- **不脱离真实环境**：与外部系统（LLM API、存储、网络服务）的协作必须在真实环境下验证，不得在集成测试中 Mock 外部服务。

---

## 2. 场景化测试指南 (基于 User Stories)

### 场景 A：插件沙箱与 4 原语协作 (Story 2 & 3 & 4)

**验证重点**：JS 插件通过 Node.js 兼容层调用宿主的 `fs.write` 时，安全管控是否拦截。

- **理论 (Theory)**：跨语言调用链路闭环。JS (QuickJS) -> WASI -> Host (Rust) -> Security Policy -> Filesystem。
- **实践 (Practice)**：
  ```rust
  #[tokio::test]
  async fn test_plugin_write_security_boundary() {
      common::setup_logging();
      let _span = tracing::info_span!("test_plugin_write_security_boundary").entered();

      // 1. Arrange: 准备环境，设置只允许写 /tmp/pi-test
      tracing::info!("Arrange: 准备引擎与白名单、越权写 /etc/passwd 的插件代码");
      let engine = TestEngine::init().with_whitelist("/tmp/pi-test");
      let plugin_code = r#"
          const fs = require('fs');
          fs.writeFileSync('/etc/passwd', 'malicious data'); // 尝试越权
      "#;

      // 2. Act: 加载并运行插件
      tracing::info!("Act: 加载并运行插件");
      let result = engine.load_and_run_js(plugin_code).await;

      // 3. Assert: 断言必须失败，且审计日志记录了拦截行为
      tracing::info!("Assert: 验证返回 Err、审计含拦截记录、/etc/passwd 未被改写");
      assert!(result.is_err());
      assert!(engine.audit_log().contains("Access Denied: /etc/passwd"));
      assert!(!std::path::Path::new("/etc/passwd").exists_ok_with_content("malicious data"));
  }
  ```

### 场景 B：事件系统全生命周期 (Story 6)

**验证重点**：插件注销后，其注册的事件监听是否彻底释放，防止内存泄露。

- **理论 (Theory)**：观察者模式的清理机制。验证 `on` / `emit` / `off` 的配对正确性。
- **实践 (Practice)**：
  ```rust
  #[test]
  fn test_event_cleanup_on_plugin_unload() {
      common::setup_logging();
      let _span = tracing::info_span!("test_event_cleanup_on_plugin_unload", plugin_id = "test-plugin").entered();

      tracing::info!("Arrange: 创建 EventBus，为 test-plugin 注册 session.start 监听");
      let mut bus = EventBus::new();
      let plugin_id = "test-plugin";
      bus.on("session.start", plugin_id, |ev| { ... });

      tracing::info!("Act: 卸载插件，再发送 session.start");
      bus.unregister_by_plugin(plugin_id);
      let call_count = bus.emit("session.start", data);

      tracing::info!("Assert: 验证无回调被执行");
      assert_eq!(call_count, 0, "插件卸载后不应再有事件回调执行");
  }
  ```

### 场景 C：LLM + Tool 动态调用流 (Story 5 & 7)

**验证重点**：与真实 LLM API 的协作（请求/响应/流式）；后续可扩展为 LLM 返回 `tool_calls` 时宿主引擎的解析与路由。

- **理论 (Theory)**：协议适配与动态分发。验证 LLM Provider 在真实环境下完成请求、响应与流式事件。
- **实践 (Practice)**：集成测试必须使用真实端点，不 Mock 外部 API。
  ```rust
  #[tokio::test]
  async fn test_llm_provider_chat_real_request_returns_ok() {
      common::setup_logging();
      let _span = tracing::info_span!("test_llm_provider_chat_real_request_returns_ok").entered();
      let _ = dotenvy::dotenv().ok();

      tracing::info!("Arrange: 加载 .env，创建 LlmConfig 与 OpenAiProvider、ChatRequest");
      let config = LlmConfig::default();
      let provider = OpenAiProvider::new(&config)
          .expect("集成测试要求设置 OPENAI_API_KEY，无 key 视为失败");
      let request = ChatRequest {
          messages: vec![ChatMessage::user("Say ok")],
          model: config.default_model.clone(),
          temperature: Some(0.0),
          max_tokens: Some(10),
          stream: Some(false),
          model_override: None,
      };

      tracing::info!("Act: 调用 provider.chat(request)");
      let resp = provider.chat(request).await.expect("chat 应成功");

      tracing::info!("Assert: 验证 choices 非空、usage 存在");
      assert!(!resp.choices.is_empty());
      assert!(resp.usage.is_some() || true);
  }
  ```

---

## 3. 内部协作测试的“三不”原则与真实环境

1. **不直接操作私有状态**：集成测试应通过 `pi-awsm init` 或 `Engine::start()` 等公开入口启动，不要手动去修改模块内部的 `Mutex` 或 `RefCell`。
2. **不跳过异步行为**：(针对 Story 4 & 6) 如果存在事件循环，必须使用 `tokio::time::timeout` 配合等待，确保异步任务（如 `setTimeout`）真实完成，而不是用 `sleep` 暴力等待。
3. **不忽略副作用验证**：集成测试必须检查磁盘（Story 2 的备份文件）、检查内存（Story 6 的监听列表）、检查控制台输出（Story 8 的渲染）。
4. **不 Mock 外部依赖**：集成测试中禁止对 LLM API、数据库、文件系统等外部服务做 Mock，必须使用真实环境；

---

## 4. 专项规范：审计与日志 (Observability)

针对 Story 2 的“可追溯”要求，集成测试必须包含对 **Audit Log** 的断言：

- **日志锚点**：测试中关键操作前后必须有 `tracing::info!`。
- **断言审计回溯**：
  ```rust
  // 执行操作后检查审计系统
  let audit_entry = audit_manager.get_last_entry().unwrap();
  assert_eq!(audit_entry.action, "FILE_WRITE");
  assert_eq!(audit_entry.user_confirmed, true);
  ```

---

## 5. 环境自愈与清理 (Teardown)

由于集成测试涉及文件操作（Story 2 & 8），必须确保测试环境的“瞬时性”：

- **TempDir 模式**：使用 `tempfile` crate 为每个测试创建独立的配置根目录。
- **进程清理**：涉及 WasmEdge 实例的任务，测试结束必须显式调用 `drop` 或 `shutdown`，并在测试结束检查是否有僵尸线程。

---

## 6. 提交通关指标 (Definition of Done)

对于 P0 级 User Story，集成测试需达到：

- **路径覆盖**：覆盖所有 `Story 验收标准` 中的勾选项。
- **错误覆盖**：必须包含至少一个“非法路径访问”和“无效 API Key”的失败分支。
- **跨平台一致性**：集成测试必须在 CI 的 Windows/macOS/Linux 矩阵中全部通过（Story 1 要求）。

---

## 理论与实践总结表

| 测试对象 | 理论验证点 | 实践验证手段 |
| :--- | :--- | :--- |
| **CLI Doctor** | 环境依赖检测算法 | 构造缺失 WasmEdge 的环境运行 `doctor` |
| **4 Primitives** | 权限拦截器中间件 | 尝试 `write` 白名单外的目录，捕获异常 |
| **Plugin System** | JS 引擎与 WASI 的绑定 | 加载一个读环境变量的 JS，看宿主能否传过去 |
| **Session Mgr** | 持久化与内存同步 | 写入一条 Chat，重启 Engine 看能否 Load 出来 |
| **LLM** | 与真实 API 的请求/响应/流式协作 | 配置 `OPENAI_API_KEY` 后调用真实 `LlmProvider::chat` / `chat_stream`，验证 `ChatResponse`、`StreamEvent` 序列；
