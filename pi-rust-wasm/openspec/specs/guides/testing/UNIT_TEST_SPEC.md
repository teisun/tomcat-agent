# 单元测试编写规范 (Unit Test Spec)

本规范与 [Constitution.md](../../Constitution.md) 中的「自测覆盖」「单测不通过则查因改码」及完成定义一致，适用于 `pi_wasm` 及本仓库内所有需单测的模块。与 [STATUS_GUIDE.md](../workflow/STATUS_GUIDE.md)、[DOCUMENTATION_GUIDE.md](../workflow/DOCUMENTATION_GUIDE.md) 风格一致。

---

## 1. 核心原则：测试即契约

- **不可跳过原则**：测试失败等同于编译失败。禁止用 `#[ignore]` 绕过逻辑错误，禁止在缺环境时静默 `return`。
- **无 key 即不通过**：依赖外部 API key（如 OPENAI_API_KEY）的用例属于核心功能，**未配置 key 时该用例须失败（panic/断言失败），不得用“跳过”绕过**；运行单测前须配置好所需环境，否则相关用例失败。
- **真实性优先**：核心 LLM 逻辑优先真实接口调用；**外部成熟 API（OpenAI 等）优先用真实请求不通再mock、**；自测时若主 API 与配置的 fallback 均不可用，**再使用 mock 数据**完成该用例，保证单测可稳定通过（优先级：真实请求 > 降级重试 > mock）。**内部未完成模块**因开发进度可用 mock。
- **自包含性**：每个用例可独立运行，不依赖其他用例的执行顺序。
- **执行与验收**：跑 `cargo test -p <crate>` 后若有失败，查原因并修复（含补全环境配置），循环直至全部通过。
- **禁止为过 CI 弱化断言**：不得以放宽断言阈值、删除或注释关键 `assert`、仅 `println!` 而不失败、无评审依据滥用 `#[ignore]` 等方式使失败用例「形式上通过」。须修复实现或重写测试，使断言真实反映契约；与 [Constitution.md](../../Constitution.md)「测试不通过则查因改码」及集成/E2E 规范一致。

### 1.2 Mock 策略：外部接口 vs 内部模块

| 测试对象 | 策略 | 说明 |
| :--- | :--- | :--- |
| 外部成熟 API (OpenAI, Anthropic) | 不通则 Mock | 真实请求；自测时主 + fallback 均不通再用 mock |
| 数据库/文件系统 | Mock/内存化 | tempfile、内存 DB 等 |
| 内部未完成模块 | Mock | Trait + 假实现 |
| 网络异常/超时 | Mock | wiremock / mockito 模拟 5xx、延迟 |

---

## 2. 环境依赖：无 key 即不通过

- 依赖外部 API key（如 `OPENAI_API_KEY`）的用例：**未配置 key 时该用例须不通过**（例如断言 `new()` 失败、或要求 key 的集成断言失败），**不得在无 key 时 return 跳过**；核心功能不能草率绕过。
- 运行单测前须在运行环境中配置好所需 key（如项目根目录 `.env`、或 `export OPENAI_API_KEY=...`），配置方式见项目说明；未配置则依赖该 key 的用例失败，需补全配置后重新跑测直至通过。
- 已配置 key 但请求仍失败（如 API 错误、超时等）：用例必须**失败**（`panic!` 或 `assert!`），不得改为“跳过”以绕过失败。
- **运行环境与网络**：依赖 OpenAI 等外部 API 的用例需在**能访问对应服务**（如 api.openai.com）的环境中运行。若在 IDE/CI 中因运行进程无外网权限而报「请求失败 / error sending request」，请**在本机终端**执行 `cargo test -p pi_wasm` 验证；通过即表示实现与配置正确。自测时降级仍不通则用 mock（见上表）。
- **强制断言环境**：涉及 `OPENAI_API_KEY` 的测试建议使用统一校验（如 `dotenvy::dotenv().ok(); std::env::var("OPENAI_API_KEY").expect("测试失败：未检测到 OPENAI_API_KEY")`），严禁静默跳过。

---

## 3. 测试分类与目录

- **单元测试**：源文件底部 `#[cfg(test)] mod tests { ... }`，覆盖私有方法、核心算法、纯函数；不应依赖外部网络，耗时低。
- **集成测试**：`tests/` 目录，覆盖公开 API、多模块协作、真实外部服务；需覆盖主业务链路。

---

## 4. 覆盖率与门禁

- **覆盖率**：核心模块 ≥85%，基础设施/工具类 ≥90%（与 [Constitution.md](../../Constitution.md) 一致）；使用 `cargo-tarpaulin` 等统计。各任务（如 [tasks_details.md](../../../changes/001-mvp/tasks_details.md)）可有单独要求（如 ≥80%）。
- 完成定义要求“单元测试通过，算出覆盖率”，写入当前分支对应的 status 文件（如 `docs/status/feature-xx.md`）；具体字段与格式见 [STATUS_GUIDE](../workflow/STATUS_GUIDE.md)（Cov% 列）。

---

## 5. 命名、结构与可读性

- **命名**：`测试对象_在何种状态下_预期结果`，如 `openai_provider_fails_on_invalid_token`、`openai_provider_new_fails_without_api_key`、`count_tokens_approximate`。
- **AAA 模式**：Arrange（准备）→ Act（执行）→ Assert（断言）；可辅以 `println!("[TEST] 开始/过程/结果: ...")` 便于排查，不要依赖打印代替断言。
- **断言**：优先 `assert_eq!` / `assert_matches!`，断言中带自定义描述；关键行为必须有断言，禁止“跑一遍不崩就算过”。复杂 AI 返回可考虑 `insta` 快照。
- **测试过程可视化 (Logging)**:
  使用 tracing 或 log 宏：严禁在生产代码中使用 println!。在测试中，推荐使用 info!、warn! 或 debug! 记录关键节点。
  记录三个关键点：
  开始 (Setup)：打印输入参数（如 Prompt 简述、模型名）。
  过程 (Processing)：打印中间状态（如收到原始响应、解析后的中间结构）。
  结果 (Result)：打印最终断言前的数据状态。
  初始化日志器：每个测试用例或测试模块开头，需初始化日志订阅者（如 tracing_subscriber），否则 Log 不会显示。
- **过程追溯 (Tracing)**
  关键路径留痕：涉及 AI 调用、复杂解析或网络请求的测试，必须在“开始前”、“收到原始结果后”、“断言前”记录日志。
  日志分级：
  info!：记录测试步骤（如：“开始调用 OpenAI API”）。
  debug!：记录原始报文或中间变量。
  error!：记录非预期的捕获异常。
  查看日志：执行测试时建议使用 `RUST_LOG=pi_wasm=debug,info cargo test -- --nocapture` 以便在开发阶段实时观察执行流。


---

## 6. Rust 单测惯例

- **同步测试**：`#[test]`；**异步测试**：统一 `#[tokio::test]`（需 `tokio` 的 `rt`、`macros` 等 feature）。
- **依赖**：仅测试用的 crate（如 `dotenvy`、`wiremock`、`insta`）放在 `[dev-dependencies]`，避免污染主依赖。
- **全局状态**：避免在测试中无谓修改全局环境（如 `std::env::set_var`）影响其他用例；必要时在用例内恢复或使用 `temp_env` 等隔离手段。
- **模块组织**：单元测试放在同文件底部 `#[cfg(test)] mod tests { ... }`，或 `tests/` 目录下集成测试；大模块可拆为 `tests/` 子目录。性能测试放 `benches/` 用 criterion。

---

## 7. 与宪法、架构的对应关系

- **宪法**：「自测覆盖」要求功能附带单测且覆盖率达标；「单测不通过则查因改码」要求不绕过失败；本规范对 mock 策略与“无 key 即不通过”的约定与宪法条文一致。
- **架构**：单测应覆盖各层对外契约（Trait、公开 API）；内部实现可用 mock 解耦依赖，见 [Codeing&Architecture_Spec.md](../coding/Codeing&Architecture_Spec.md) 中的可测性与依赖反转。

---

## 8. 提交检查清单

在执行 `git commit` 前自检：

1. 是否有新增功能？若有，必须附带至少一个测试用例。
2. 本地 `cargo test` 是否全量通过？
3. 是否存在被 `#[ignore]` 的遗留问题？
4. 涉及 Key 的测试是否在本地（或私有 CI）验证通过？
5. 是否仅保留关键链路日志，清空不必要的 `println!`？

存量代码可遵循“童子军军规”：每次修改到某模块时顺手补齐该模块规范测试；CI 中测试通过后方可合并。

---

## 9. 参考

- [Constitution.md](../../Constitution.md) 第二节「Agent 协作规范」、第三节「完成定义」
- [STATUS_GUIDE.md](../workflow/STATUS_GUIDE.md)、[DOCUMENTATION_GUIDE.md](../workflow/DOCUMENTATION_GUIDE.md)（文档与进度规范）
- 各 Agent 任务中的验收标准（如 llm_agent、tasks_details 中的覆盖率与边界用例要求）
