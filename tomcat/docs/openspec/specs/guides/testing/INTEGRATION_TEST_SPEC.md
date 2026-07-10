这份《集成测试规范》旨在为开发团队提供一套标准化的测试流程和准则。Rust 的集成测试主要关注**外部接口（Public API）**、**模块间交互**以及**外部依赖（数据库、网络等）**。

---

# Rust 程序集成测试规范 (v1.0)

## 0. 文档结构

本规范为集成测试主文档，以下为全部门槛与参考章节。第 9、10、11 章因内容较多单独成子文档。

| 章 | 标题 |
|----|------|
| 1 | 测试目标 |
| 2 | 目录结构与组织 |
| 3 | 命名规范 |
| 4 | 测试编写标准 |
| 5 | 外部依赖处理 |
| 6 | 断言与工具库 |
| 7 | 执行与持续集成 (CI) |
| 8 | 最佳实践建议 |
| 9 | 日志与链路追踪规范 → 子文档 [INTEGRATION_TEST_LOGGING.md](INTEGRATION_TEST_LOGGING.md) |
| 10 | 鲁棒性保障：异常与边界（门禁）→ 子文档 [INTEGRATION_TEST_ROBUSTNESS.md](INTEGRATION_TEST_ROBUSTNESS.md) |
| 11 | 实践参考：场景与示例 → 子文档 [INTEGRATION_TEST_PRACTICE.md](INTEGRATION_TEST_PRACTICE.md) |
| E2E | E2E 测试规范 → 子文档 [E2E_TEST_SPEC.md](E2E_TEST_SPEC.md)（场景库 → [E2E_SCENARIO_LIBRARY.md](E2E_SCENARIO_LIBRARY.md)） |

---

## 1. 测试目标
*   所有测试均需要编写独立的测试代码，不是复用单元测试代码
*   验证多个模块组合后的行为是否符合预期。
*   确保从外部（作为 Crate 使用者）调用公共接口时的正确性。
*   验证程序与外部系统（如数据库、文件系统、第三方 API）的集成。
*   **集成测试不得脱离真实环境**：验证与外部系统（第三方 API、数据库、文件系统等）的协作时，必须使用真实依赖而非 Mock；外部协作与模块间协作均属“协作”，均需在真实环境下验证。
*   **须覆盖异常与边界**：集成测试除主路径外，须覆盖异常场景与边界条件（环境/契约/状态边界），详见第 10 章 [集成测试鲁棒性保障](INTEGRATION_TEST_ROBUSTNESS.md)。

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

### 2.3 仓库内 scratch 目录

若必须在仓库树内写临时文件（集成/E2E harness、本地复现）：统一放在 **`tomcat/workspace-temp/`** 下按子目录分用途（`temp` 后缀，与产品里的 `agent_definition_dir` / `tomcat workspace` 等「workspace」语义区分）；该目录已在 `tomcat/.gitignore` 中忽略。不要在 `tomcat/` 根目录散落 `tool-results/`、`tmp/` 等；**不要**使用 crate 下的 `workspace/` 作为 scratch（易与提示词里的「workspace」混淆）。

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
*   **异常/故障注入**：为验证重试、退避、熔断等行为，允许在集成测试中使用 Mock Server（如 wiremock）模拟外部服务失败（如 429、503、超时、连接重置）。此类用例与「真实 API 协作」用例并存，且应在文档或用例命名中明确标注为「故障注入」用途；详见第 10 章 [集成测试鲁棒性保障](INTEGRATION_TEST_ROBUSTNESS.md)。
*   **代码级 Mock**：单元测试中对于难以集成的逻辑，可使用 `mockall` 宏生成 Mock 对象；集成测试中不 Mock 外部服务。

### 5.3 真实环境要求
*   集成测试以真实环境为默认：与外部系统（数据库、第三方 API、文件系统等）的协作必须在真实环境下验证。
*   Mock 仅用于单元测试或尚未完成建设的内部模块；集成测试套件中必须包含与真实外部依赖协作的用例（如 LLM 的 `llm_tests.rs`）；（无 key 或不可达时要求见 5.2）。
*   **E2E 测试**（进程边界黑盒 + 用户操作模拟）的定义、场景库与覆盖规则见 [E2E_TEST_SPEC.md](E2E_TEST_SPEC.md)；完整用户场景清单见 [E2E_SCENARIO_LIBRARY.md](E2E_SCENARIO_LIBRARY.md)；新功能合并时须符合其 §6 强制覆盖规则。

### 5.4 插件运行时（真实 `rquickjs`）
*   **插件相关集成测试**须覆盖真实 `rquickjs` 运行时：至少有一组测试直接使用 `PluginEngine` / `PluginVmInstance` / `PluginManager` / `VmActor` 执行脚本、分发事件并断言宿主侧行为（如 hostcall 被调用、runtime 状态转换符合预期）。
*   当前主验收入口是 `tests/quickjs_e2e_tests.rs`、`tests/long_lived_vm_tests.rs` 以及 `src/ext/plugin/tests/suite_test.rs` 中的集成风格用例；覆盖脚本执行、bridge、shims、crypto、panic/超时隔离、多 session 隔离与机会式 idle 回收。
*   **环境缺失不允许跳过或绕过**。若本机构建缺少 `rquickjs` 所需前置条件，须先修复环境再执行；不得把插件运行时用例改成只测 mock。
*   **失败即失败**：上述构建或测试若失败，视为集成测试不通过，不得以「环境未就绪」为由跳过或记录为通过。
*   **不得通过降低断言或放宽验收条件使用例通过**：不得通过降低断言或放宽「宿主侧行为」（如 hostcall 被调用、返回符合预期、runtime 正确回收/隔离）的验收条件来使用例通过；若运行时/环境不满足要求，须查因修复或记录阻塞，用例视为不通过（见 Constitution 第 24 条）。
*   与 5.2 中 LLM 真实 API 要求并列：`rquickjs` 运行时与 LLM 均属于需要真实链路覆盖的核心外部/运行时依赖。

## 6. 断言与工具库
推荐集成以下工具以增强测试表达力：
*   **`pretty_assertions`**：在断言失败时提供更易读的 Diff。
*   **`rstest`**：支持参数化测试（类似于 Pytest 的 parametrize）。
*   **`claims`**：专门用于对 `Result` 和 `Option` 进行优雅断言。

## 7. 执行与持续集成 (CI)

### 7.1 本地执行
*   **分类执行约定（本仓库）**：Rust 门禁唯一入口仍是 `./scripts/run-integration-tests.sh`；`integration*` 统一走 `cargo-nextest`，`lib` / `doctest` 继续用 `cargo test`，避免把纯单测误切到“一用例一进程”模型。
*   运行默认集成测试：`RUST_LOG=tomcat=debug,info ./scripts/run-integration-tests.sh integration`
*   仅运行默认 integration 组：`RUST_LOG=tomcat=debug,info ./scripts/run-integration-tests.sh integration-parallel`
*   仅运行 serial 兜底组（默认空）：`RUST_LOG=tomcat=debug,info ./scripts/run-integration-tests.sh integration-serial`
*   运行真 LLM 显式层：`RUST_LOG=tomcat=debug,info ./scripts/run-integration-tests.sh integration-real-llm`
*   运行 Rust 快门禁：`RUST_LOG=tomcat=debug,info ./scripts/run-integration-tests.sh gate-fast`
*   运行 Rust 全门禁：`RUST_LOG=tomcat=debug,info ./scripts/run-integration-tests.sh gate-full`
*   运行特定文件：默认目标可用 `RUST_LOG=tomcat=debug,info cargo nextest run --test audit_tests`；显式 real-llm 目标用 `RUST_LOG=tomcat=debug,info cargo nextest run --profile real-llm --test plan_real_llm_cli_e2e`。若要覆盖 doctest，额外执行 `cargo test --doc`。
*   仅运行库内单元测试：`RUST_LOG=tomcat=debug,info cargo test --lib`
*   仅运行 doctest：`cargo test --doc`

### 7.2 并发模型、默认过滤与显式层
集成测试二进制名单维护在 `scripts/test-groups.sh`，nextest 过滤 / test-group 维护在 `.config/nextest.toml`。两者必须同步，缺一不可。

**当前模型（默认追求最省时间）**：

1. **默认满并发车道**：`cargo nextest run` 的默认 `test-threads = num-cpus`。凡已证实不共享进程级全局状态或真实 `~/.tomcat` 盘目录的 binary，全部放这里，包括原先的 `cli_tests`、`checkpoint_cli_e2e`、`resume_hydration_cli_e2e`、`quickjs_e2e_tests`、`hostcall_tests`、`long_lived_vm_tests`、`primitives_tools_tests`、`tool_catalog_doc`、`serve_*` 等。
2. **serial 兜底组**：`test-groups.serial.max-threads = 1`，默认空。只有 3x 连跑证明确实互踩、OOM 或明显抖动时，才把个别 binary 临时回退到这里。
3. **real-llm 显式层**：`.config/nextest.toml` 的 `profile.real-llm` 负责收口 `TOMCAT_INTEGRATION_REAL_LLM_TESTS` 以及 `cli_tests` 里 `*_real_llm_cli` 慢用例；其 `test-groups.real-llm.max-threads = 2` 只为规避 provider API 限流，不是正确性要求。

**隔离原则**：

*   `nextest` 是“一用例一进程”，因此 `set_var` / `cwd` 这类进程内状态不再天然要求整组 `-j1 --test-threads=1`。
*   真 LLM / CLI E2E 里凡会落盘到 `~/.tomcat` 的路径，都必须在测试入口先切到**临时 HOME**，让 `~/.tomcat/*` 落到私有 tempdir；默认门禁不再依赖真实家目录。
*   默认过滤（`profile.default.default-filter`）会排除显式 real-llm binary 与 `cli_tests` 里的 `*_real_llm_cli` 慢用例；它们只能通过 `integration-real-llm` / `profile real-llm` 进入门禁。

### 7.3 CI 检查项
在流水线（如 GitHub Actions）中，集成测试应包含：
1.  **代码风格检查**：`cargo fmt --check`
2.  **静态分析**：`cargo clippy`
3.  **漏洞扫描**：`cargo audit`
4.  **覆盖率要求**：集成测试应覆盖核心业务路径（使用 `cargo-tarpaulin` 统计）。

### 7.4 全量集成测试
默认门禁与分层流程如下：

*   **快门禁**（默认 / `all` / `gate-fast`）：`clippy` → `lib(cargo test)` → `doctest` → `integration(nextest 默认满并发)`  
    命令：`RUST_LOG=tomcat=debug,info ./scripts/run-integration-tests.sh gate-fast`
*   **全门禁**（合并前显式一次）：`gate-fast` → `integration-real-llm`  
    命令：`RUST_LOG=tomcat=debug,info ./scripts/run-integration-tests.sh gate-full`
*   **release**：不再属于默认门禁；只在确需产物时单独执行  
    命令：`./scripts/run-integration-tests.sh release`

## 8. 最佳实践建议
*   **不要过度 Mock**：集成测试的价值在于真实性，如果所有外部依赖都被 Mock 了，那它就变成了单元测试。
*   **环境一致性**：确保开发环境、测试环境和 CI 环境的基础设施版本（如 PostgreSQL 版本）完全一致。
*   **测试清理**：利用 `Drop` trait 或特定的清理脚本，确保测试产生的临时文件或数据被及时删除。
*   **注释**：对于复杂的集成逻辑，必须在测试函数头部注释说明该测试的业务场景。
*   **DoD 要点**：路径覆盖、错误覆盖、跨平台一致性、异常与边界覆盖、审计可追溯。详细条款见第 11 章 [集成测试实践参考](INTEGRATION_TEST_PRACTICE.md)。



## 9. 日志与链路追踪规范

为保证失败可定位、行为可追溯，**每个集成测试用例必须同时满足**以下门禁（集成测试门禁之一）：

### 9.0 强制要求（集成测试门禁）

1. **初始化**：用例入口调用 `common::setup_logging()`（或共享入口调用一次），避免重复 init（使用 `Once`）。
2. **上下文**：为每个测试用例创建 `info_span!`（或使用 `#[instrument]`）标注用例名与关键参数（如 plugin_id、session_key）。
3. **AAA 日志锚点**：在 Arrange / Act / Assert 三个阶段的关键步骤**至少各记录一条** `tracing::info!`（必要时补 `tracing::debug!` 记录关键变量）。

> 说明：默认 `cargo test` / `cargo nextest run` 都会捕获输出；若需要在诊断场景里实时查看逐步日志，可用 targeted `cargo test -- --nocapture` 或读取脚本产出的日志文件。详细技术说明与目录结构见 [INTEGRATION_TEST_LOGGING.md](INTEGRATION_TEST_LOGGING.md)。

---

## 10. 鲁棒性保障：异常与边界

**集成测试门禁**：全量集成测试须包含并通过鲁棒性/异常边界类用例（如 `robustness_tests` 或等价的异常、边界、超时、资源类用例）；具体要求与清单见子文档 [集成测试鲁棒性保障](INTEGRATION_TEST_ROBUSTNESS.md)。

- **鲁棒性编写要求**：须包含并维护 `robustness_tests.rs`（或等效的异常、边界、超时、资源泄露等用例），符合 [INTEGRATION_TEST_ROBUSTNESS.md](INTEGRATION_TEST_ROBUSTNESS.md) 要求。
- **鲁棒性验证门禁**：全量 `RUST_LOG=tomcat=debug,info ./scripts/run-integration-tests.sh integration` 须包含并通过鲁棒性用例（如并发组中的 `--test robustness_tests`）；不满足则与日志门禁同样处理（补全后再跑全量验收）。
- **验收清单项**：**鲁棒性集成测试**：`RUST_LOG=tomcat=debug,info cargo test --test robustness_tests -- --nocapture` 通过（或等价地，`RUST_LOG=tomcat=debug,info ./scripts/run-integration-tests.sh integration` 已包含 robustness_tests 并通过）。

集成测试须覆盖异常与边界场景（环境/契约/状态边界），包括故障注入、超时控制、脏数据与非法路径、资源泄露验证及异常测试的断言准则与清单。正文见子文档 [集成测试鲁棒性保障](INTEGRATION_TEST_ROBUSTNESS.md)。

---

## 11. 实践参考：场景与示例

基于 User Story 的场景化测试（插件沙箱与 4 原语、事件系统、LLM + Tool）、三不原则、审计与 Teardown、提交通关指标 (DoD) 及总结表。正文见子文档 [集成测试实践参考](INTEGRATION_TEST_PRACTICE.md)。
