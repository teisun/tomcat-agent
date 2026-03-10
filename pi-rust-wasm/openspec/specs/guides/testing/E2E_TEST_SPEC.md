# E2E 测试规范

本文档定义 pi-rust-wasm 项目的端到端（E2E）测试类别、场景来源、编写标准与执行规范。完整用户操作模拟场景清单见 [E2E_SCENARIO_LIBRARY.md](E2E_SCENARIO_LIBRARY.md)。

---

## §1 定义与范围

E2E 测试 = **进程边界黑盒 + 用户操作模拟**：

- 启动 `pi` 命令（二进制 `pi`，crate `pi_wasm`）或真实 WasmEdge + QuickJS 运行时
- 模拟真实用户的完整操作路径（输入 → 等待 → 断言输出/副作用）
- 断言对象：stdout/stderr/exit code/磁盘文件/进程行为

### 三层对比

| 维度   | 单元测试   | 集成测试          | E2E 测试                      |
|--------|------------|-------------------|-------------------------------|
| 测试视角 | 函数/结构体 | pub API 调用者    | 终端用户                      |
| 测试边界 | 源码内     | 进程内（pub API） | 进程外（子进程/运行时）       |
| 断言对象 | 返回值/状态 | 数据结构/行为     | stdout/stderr/文件/exit code  |

项目内两种 E2E：

- **CLI E2E**：`tests/cli_tests.rs`，通过 `assert_cmd` 启动 `pi` 子进程
- **Wasm E2E**：`tests/wasmedge_e2e_tests.rs`，驱动真实 WasmEdge + QuickJS 运行时

---

## §2 用户操作模拟场景库

完整场景库见 **[E2E_SCENARIO_LIBRARY.md](E2E_SCENARIO_LIBRARY.md)**，共 39 条，覆盖 P0 全部 User Stories。

新增用例须同步更新 `E2E_SCENARIO_LIBRARY.md`，并遵循编号规则（`E2E-CLI-NNN` / `E2E-WASM-NNN`）。

---

## §3 编写标准

### 用户操作模拟强制要求

- 每个 E2E 用例**必须有用户意图描述**，写在函数头 `///` 注释第一行，格式：`/// [用户场景] 用户 <想做什么>`
- 断言必须覆盖用户可见的 stdout 内容，不得仅断言 exit code
- 涉及 AI 回复的用例，须断言 stdout 非空或含特定关键词（如「所有权」、代码块标识）

### 技术约束

- **CLI E2E**：`Command::cargo_bin("pi")`，禁止直接调用 Rust pub API
- **Wasm E2E**：`WasmEngine::global()` + `WasmInstance`，须真实 WasmEdge
- **环境隔离**：每用例 `tempfile::tempdir()` + `PI_WASM__STORAGE__WORK_DIR` 环境变量隔离
- **文档注释**：三行格式（`[用户场景]` / 验证 / 意义）
- **日志**：`common::setup_logging()` + `info_span!` + AAA 三阶段各一条 `tracing::info!`
- **超时**：chat/Wasm 类用例须 `.timeout(Duration::from_secs(60))` 或 `tokio::time::timeout`

---

## §4 不可跳过原则

- 无 `OPENAI_API_KEY` 时 chat 类 E2E 须 `panic!`（不得 `#[ignore]`）
- 无 WasmEdge 时 Wasm E2E 须 `panic!` 并给出安装命令（`./scripts/install-wasmedge.sh`）
- 失败即失败，不得以「环境未就绪」为由跳过

---

## §5 执行规范（统一使用 RUST_LOG）

所有 E2E 测试命令**必须**包含 `RUST_LOG=pi_wasm=debug,info`，确保 INFO/DEBUG 日志全部输出：

```bash
# CLI E2E（含全日志）
RUST_LOG=pi_wasm=debug,info cargo test --test cli_tests -- --nocapture

# Wasm E2E（含全日志）
RUST_LOG=pi_wasm=debug,info cargo test --test wasmedge_e2e_tests -- --nocapture

# 一键全量（含 E2E，含日志）
./scripts/run-integration-tests.sh
```

---

## §6 新功能 E2E 覆盖规则（强制）

**每次合并新功能时，必须在 `tests/cli_tests.rs` 或 `tests/wasmedge_e2e_tests.rs` 中补充至少 1 条用户操作模拟用例。**

判断标准（满足其一即须补）：

- 新增或修改了任何 `pi` CLI 子命令
- 新增或修改了用户可见的 chat/plugin/session/config/audit 行为
- 新增了 Wasm/插件相关能力

补充用例须：

1. 使用子进程 `Command::cargo_bin("pi")`（CLI 类）或 Wasm 运行时（Wasm 类）
2. 函数名前缀 `test_user_` 表示用户视角
3. 断言 stdout 内容（不得仅断言 exit 0）
4. 同步更新 [E2E_SCENARIO_LIBRARY.md](E2E_SCENARIO_LIBRARY.md)

---

## §7 覆盖要求与 DoD

- P0 User Stories 的所有「用户可 X」验收标准，须有对应 `test_user_*` E2E 用例
- 每次 Nibbles 集成循环，E2E 步骤必须全部通过，不可跳过
- 新用例须通过 `RUST_LOG=pi_wasm=debug,info cargo test --test cli_tests -- --nocapture` 验证日志可见
