# 集成与 E2E：交付步骤与验收

## 适用场景

| 角色 | 时机 | 说明 |
|------|------|------|
| 工程师（Tom/Jerry/Spike） | 标 `PENDING_INTEGRATION` 前，在功能分支 | 完成 §1–§3 与 §4 全量验收 |
| Nibbles | 合并到 `develop` 后 | 按本文相同顺序与命令复跑与验收 |

**TASK_BOARD.md** 中 `PENDING_INTEGRATION` 状态说明引用本文档。

---

## 质量红线（必遵）

- **本次变更闭环**：集成测试与 E2E **暴露的问题 / bug 须在交付前修复完毕**；不得把已知失败留到后续环节「兜底」。
- **禁止降级通过**：**不得**为通过 CI 或本地门禁而 **弱化断言**（放宽阈值、删除/注释关键 `assert`、以打印代替失败、滥用 `#[ignore]`、无评审依据跳过用例等）。单测、集成测试、E2E 均适用；与 [Constitution.md](../openspec/specs/Constitution.md) 及 [UNIT_TEST_SPEC.md](../openspec/specs/guides/testing/UNIT_TEST_SPEC.md) 一致。
- **规范依据**：集成测试须符合 [INTEGRATION_TEST_SPEC.md](../openspec/specs/guides/testing/INTEGRATION_TEST_SPEC.md)（含第 9、10 章门禁）；E2E 须符合 [E2E_TEST_SPEC.md](../openspec/specs/guides/testing/E2E_TEST_SPEC.md)。

---

## 测试执行策略：子 Agent 跑测试 + 主 Agent 监控

集成测试与 E2E 测试可能因死锁、无限循环、外部依赖超时等原因**长时间挂起不返回**。为避免浪费时间和阻塞交付流程，**所有 §2–§4 中的测试执行**必须采用以下模式：

### 执行模式

1. **主 Agent 启动子 Agent（或后台 Shell）执行测试命令**：将 `cargo test ...` 等测试命令交由子 Agent / 后台终端执行，主 Agent **不得阻塞等待**。
   - **推荐模板**（项目根为 `pi-rust-wasm/`，日志便于轮询与留存）：

```bash
cd pi-rust-wasm
RUST_LOG=pi_wasm=debug,info cargo test -j 1 -- --nocapture --test-threads=1 \
  > .integration_test_output.log 2>&1 &
TEST_PID=$!
# 轮询：反复执行 tail -80 .integration_test_output.log（间隔按指数退避 5s→10s→20s→30s，上限 30s）
# 超时：kill $TEST_PID（单用例约 120s 无新输出、全量约 10 分钟仍不结束则介入，见下文）
# 结束判定：日志文件末尾出现 EXIT_CODE=0 为通过；非 0 为失败
```

### 常见错误（须避免）

以下做法容易导致**看不到进度、误判卡死、或被工具中途 Abort**，与「全量集成/E2E」的真实耗时完全不匹配：

1. **在前台直接跑全量 `cargo test`（尤其通过 IDE/Agent 单次调用、且带较短 block 超时）**  
   全量会先 **长时间编译**（常达数分钟），再顺序执行 lib、多 crate 集成测试、`cli_tests`（含真实 LLM 请求的用例）、`wasmedge_e2e_tests` 等。调用方若因超时把进程 **Abort**，终端里往往**几乎没有可读的累计输出**，容易误以为「挂住」或「没跑起来」。

2. **用「管道 + tail」试探全量测试**（例如 `cargo test ... 2>&1 | tail -5`）  
   在进程结束之前管道下游**读不到完整流**，表现为长时间无输出；同样无法观察**当前跑到哪一个测试**。

3. **把「全量测试」当成「应快速失败」的探测命令**  
   快速失败只适用于**缩小范围**后的单测（如 `cargo test -j 1 某测试名 -- --test-threads=1`）。**§4 全量门禁**必须按上文模板**写日志文件 + 后台跑**，再通过 `tail -f .integration_test_output.log` 或周期性 `tail -80 .integration_test_output.log` 观察；**不得以「先前台跑一下看会不会马上挂」代替正规验收**。

4. **正确习惯**：全量一律 **重定向到 `pi-rust-wasm/.integration_test_output.log`（已 gitignore）**、`&` 后台、`echo EXIT_CODE=$? >>` 收尾；需要时 `kill $(jobs -p)` 或记录 `$!` 后 `kill` 中止。

2. **主 Agent 持续轮询监控**：主 Agent 通过读取终端输出文件（**优先**打开 `.integration_test_output.log`），按指数退避（如 5s → 10s → 20s → 30s，上限 30s）轮询检查测试进度。
3. **超时判定与介入**：
   - **单测试用例超时**：若某个测试用例持续 **120 秒**无新输出且未完成，判定为卡住。
   - **全量测试超时**：若整个测试进程持续 **10 分钟**仍未结束，判定为整体超时。
   - 达到超时阈值后，主 Agent 须**立即**：
     1. **中止测试进程**（`kill` 对应 PID）。
     2. **完整保存终端输出日志**到 `.integration_test_output.log`。
     3. **分析日志**：定位卡住的测试用例名称、最后输出行、可能原因（死锁 / 无限循环 / 外部依赖等）。
     4. **排查与修复**：根据分析结果修复代码或测试中的问题（如加超时、修死锁、mock 外部依赖）。
     5. **重新执行测试**：修复后再次以同样的子 Agent 模式重跑，直至全部通过或确认为已知环境限制。

### 日志要求

- 每次测试执行（含因超时中止的）均须在终端输出中保留完整日志。
- 卡住时的诊断结论须记录在对应的 `docs/status/` 状态文件或提交信息中，说明**卡在哪里、为什么、如何修复**。

### 禁止行为

- **禁止**主 Agent 发起测试命令后无限阻塞等待结果。
- **禁止**对**全量** `cargo test` 使用「前台短时跑一下 / 管道 tail / 指望立刻失败」代替写日志 + 后台 + 轮询（见上文「常见错误」）。
- **禁止**发现测试卡住后仅跳过（`#[ignore]`）或删除该用例而不排查根因。
- **禁止**在未确认测试全部通过的情况下标记 `PENDING_INTEGRATION`。

### OpenAI API Key 与 LLM / CLI 集成测试（配置要点）

`cli_tests`、`llm_tests` 及部分 E2E 用例依赖 **真实** `OPENAI_API_KEY` 与可达的 `api.openai.com`（见 [INTEGRATION_TEST_SPEC.md](../openspec/specs/guides/testing/INTEGRATION_TEST_SPEC.md) §5.2）。以下做法在本地已验证可稳定跑通上述测试：

1. **密钥文件位置**：在 **`pi-rust-wasm/.env`**（crate 根，与 [`scripts/verify-openai-apis.sh`](../scripts/verify-openai-apis.sh) 使用的 `ROOT_DIR/.env` 为同一路径）中配置至少：
   - `OPENAI_API_KEY=...`（必填）
2. **代理（按需）**：若直连上游不可用，在同一 `.env` 中配置 `HTTPS_PROXY`（及必要时 `HTTP_PROXY`）。`reqwest` 会读取当前进程环境；`llm_tests` 日志中可见代理是否生效。
3. **先探活再跑长测**：在跑全量或 `cli_tests` 前，可在 `pi-rust-wasm` 下执行 `./scripts/verify-openai-apis.sh 1 3`（或交互选 `1`～`5`），用同一 `.env` 快速验证 key 与网络，避免 `cargo test` 编译许久后才因 401/网络失败。
4. **向测试进程注入环境（推荐）**：仅依赖测试内的 `dotenvy::dotenv()` 时，cwd 非 crate 根可能加载不到 `.env`。**推荐**在 shell 中与 verify 脚本一致先导出变量再跑测试：

```bash
cd pi-rust-wasm
set -a
# shellcheck disable=SC1091
source .env
set +a
cargo test -j 1 -p pi_wasm --test cli_tests --test llm_tests -- --nocapture --test-threads=1
```

5. **与 §4 全量门禁衔接**：仍需遵守上文「写日志 + 后台 + 轮询」模板时，可将上述 `cargo test` 换为全量命令并重定向到 `.integration_test_output.log`；若环境变量仅存在于当前 shell，须在**同一** `source .env` 后的子 shell 内启动后台测试，避免子进程丢失 `OPENAI_API_KEY`。

---

## 交付顺序（不可颠倒）

先 **§1 规格与场景库**，再 **§2 集成测试代码**，再 **§3 E2E 测试代码**，最后 **§4 全量测试与验收清单**。

### §1 检查并补充 User_Stories 与 E2E_SCENARIO_LIBRARY（先做）

- **时机**：本次变更相关实现已完成，在编写或大规模改动集成/E2E 测试代码**之前**。若此前已完成规格更新且与当前代码一致，以**核对与补漏**为主；若存在合并/冲突引入的偏差或新增变更，须**补充或修正**。
- **依据**：本次变更引入的能力，以及 [Architecture.md](../openspec/specs/Architecture.md) 及其子文档中的相关技术方案。
- **动作**：
  - **User_Stories.md**：若本次变更实现或变更了某 P0/P1 用户故事相关能力，则补充或更新对应描述与验收标准，使其与当前实现一致。
  - **E2E_SCENARIO_LIBRARY.md**：若引入了新的用户可见操作或场景，则补充或更新 E2E 用例表（编号、用例名、用户意图、操作序列、必须断言）。无变更则无需修改。
- **自检**：规格与场景库与本次变更实现一致、无遗漏；再进入 §2、§3。

### §2 编写集成测试代码

- **时机**：在 §1 之后；未完成不得进入 §4。
- **依据**：[INTEGRATION_TEST_SPEC.md](../openspec/specs/guides/testing/INTEGRATION_TEST_SPEC.md)、[INTEGRATION_TEST_PRACTICE.md](../openspec/specs/guides/testing/INTEGRATION_TEST_PRACTICE.md)。
- **动作**：针对本次变更引入的模块与场景，在 `tests/` 下建立或更新集成测试文件，**仅通过 `pub` API** 做黑盒测试。
- **Wasm 真实运行时**：若涉及插件/Wasm 加载或运行时，须包含「Wasm 真实运行时」集成测试；实现前阅读 **INTEGRATION_TEST_SPEC 5.4** 与 **Constitution 二、3**（测试不得假绿；Wasm 门禁见该规范）。
- **静态检查**：进入 §4 全量验收前须通过 `cargo clippy --all-targets -- -D warnings`（可与 §4 第 1 项一并执行；覆盖 `tests/` 且警告即失败）。
- **验证**：`RUST_LOG=pi_wasm=debug,info cargo test -j 1 --test '*' -- --nocapture --test-threads=1` 通过。

**检查清单**：

- [ ] 列出本次变更新增/变更的对外能力与主流程、边界场景。
- [ ] 对照 `tests/` 确认均有黑盒集成覆盖；「从磁盘/路径加载并验证行为」类须有端到端级断言（如 `load_plugin` 后在 list_loaded 或可响应事件）。
- [ ] 无覆盖则补齐，不得以「后续环节再写」为由跳过。
- [ ] 若测试涉及 JS/Wasm 执行，确认 JS 侧 `FATAL`/`ASSERT FAILED` 日志均有对应 Rust 断言兜底，避免假绿。

### §3 编写 E2E 测试代码

- **时机**：在 §1、§2 之后。
- **依据**：[User_Stories.md](../openspec/specs/User_Stories.md)、[E2E_SCENARIO_LIBRARY.md](../openspec/specs/guides/testing/E2E_SCENARIO_LIBRARY.md)、[E2E_TEST_SPEC.md](../openspec/specs/guides/testing/E2E_TEST_SPEC.md)。
- **动作**：根据场景库中与本次变更相关的用例，在 `tests/cli_tests.rs` 或 `tests/wasmedge_e2e_tests.rs` 中编写或补充 `test_user_*` / Wasm E2E 用例，与场景库对应。
- **验证**：`RUST_LOG=pi_wasm=debug,info cargo test -j 1 --test cli_tests -- --nocapture --test-threads=1`；环境具备时执行 `RUST_LOG=pi_wasm=debug,info cargo test -j 1 --test wasmedge_e2e_tests -- --nocapture --test-threads=1`。

### §4 全量测试与验收清单

完成 §1–§3 后须通过以下全量验收。**一键（可选）**：`./scripts/run-integration-tests.sh`（含 `release` → `clippy` → `lib` → `integration`，与下方自动化门禁对齐）。

以下为全量验收依据；细节与门禁另见 [INTEGRATION_TEST_SPEC.md](../openspec/specs/guides/testing/INTEGRATION_TEST_SPEC.md) 第 7、9、10 章。

#### 自动化门禁（脚本/测试，必须 pass）

1. **构建与静态检查**：`cargo build --release`、`cargo clippy --all-targets -- -D warnings`、`RUST_LOG=pi_wasm=debug,info cargo test -j 1 -- --nocapture --test-threads=1`
2. **CLI 子命令**：`pi init`、`pi doctor`、`pi config`、`pi session`、`pi plugin`、`pi audit` 可执行且帮助完整
3. **集成测试（含门禁）**：`RUST_LOG=pi_wasm=debug,info cargo test -j 1 --test '*' -- --nocapture --test-threads=1` 通过，含日志门禁与鲁棒性集成测试
4. **E2E**：`RUST_LOG=pi_wasm=debug,info cargo test -j 1 --test cli_tests -- --nocapture --test-threads=1` 通过；WasmEdge 已就绪时 `RUST_LOG=pi_wasm=debug,info cargo test -j 1 --test wasmedge_e2e_tests -- --nocapture --test-threads=1` 通过；须符合 E2E_TEST_SPEC §6
5. **Wasm 真实运行时（若任务涉及插件/Wasm）**：按 INTEGRATION_TEST_SPEC 5.4 执行

**WasmEdge stderr 说明**：VM cleanup 阶段日志中可能出现 `[error] execution failed: host function failed, Code: 0x8d`（event channel 关闭后宿主调用返回错误）。**这不代表测试失败**；是否通过以 Rust test harness 输出的 `ok` / `FAILED` 为准。

#### 人工验收（条件具备时）

6. **对话模式**：`pi chat` 可进入对话；流式输出、多轮上下文、会话切换、4 原语/工具调用与用户确认
7. **插件**：可加载/卸载 pi-mono 风格插件，错误隔离、工具与事件清理正常
8. **跨平台**：若条件具备，在 Windows/macOS/Linux 至少各跑一次 build + test
