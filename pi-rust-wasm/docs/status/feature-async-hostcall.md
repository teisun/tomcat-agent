| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Jerry | 2026-03-11 | DONE | feature/async-hostcall | - |

### 本次执行说明（HostApiDispatcher 模块文档图）

- [✓] **dispatcher.rs**：在模块级文档中新增三处 ASCII 图——结构示意（Processor 与异步基础设施）、调用流（dispatch/dispatch_async 分支与路由）、异步 submit/poll 时序，便于理解分发器职责与调用链。

---

| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Jerry | 2026-03-11 | DONE | feature/async-hostcall | - |

### TASK-12 异步 Hostcall 与 submit/poll 机制实现

- [✓] 8.4.1 `AsyncCallStatus` 枚举 + `async_results: Arc<DashMap>` + `tokio_handle` 字段
- [✓] 8.4.2 `dispatch()` 改造：callId 非空时 spawn Tokio 异步任务，立即返回 `{pending: true}`
- [✓] 8.4.3 `__async.poll` 路由：从 `async_results` 查询结果，ready 后自动清理
- [✓] 8.4.4 共享 Tokio Handle：`dispatch()` 同步路径改用 `Handle::block_on()` 替代 `Runtime::new()`
- [✓] 8.4.5 异步任务超时：`tokio::time::timeout` 包裹，默认 30s，可配置
- [✓] 8.4.6 实例销毁清理：`cleanup_instance()` 方法 + `instance_calls` 映射
- [✓] 8.4.7 LLM Semaphore 限流（默认 5 并发）
- [✓] 8.4.8 10 个单元测试全部通过（全链路、超时、并发、清理、poll 错误等）
- [✓] 全量单元测试 234 passed，clippy 无新 warning

### 涉及文件
- `Cargo.toml`：新增 `dashmap = "6"`
- `src/ext/dispatcher.rs`：核心改动（异步任务管理 + poll 路由 + Semaphore）
- `src/ext/mod.rs`：导出 `AsyncCallStatus`
- `agents/TASK_BOARD.md`：状态更新
