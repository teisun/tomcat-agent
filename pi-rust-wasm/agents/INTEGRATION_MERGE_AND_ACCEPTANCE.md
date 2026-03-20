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
- **验证**：`RUST_LOG=pi_wasm=debug,info cargo test --test '*' -- --nocapture` 通过。

**检查清单**：

- [ ] 列出本次变更新增/变更的对外能力与主流程、边界场景。
- [ ] 对照 `tests/` 确认均有黑盒集成覆盖；「从磁盘/路径加载并验证行为」类须有端到端级断言（如 `load_plugin` 后在 list_loaded 或可响应事件）。
- [ ] 无覆盖则补齐，不得以「后续环节再写」为由跳过。

### §3 编写 E2E 测试代码

- **时机**：在 §1、§2 之后。
- **依据**：[User_Stories.md](../openspec/specs/User_Stories.md)、[E2E_SCENARIO_LIBRARY.md](../openspec/specs/guides/testing/E2E_SCENARIO_LIBRARY.md)、[E2E_TEST_SPEC.md](../openspec/specs/guides/testing/E2E_TEST_SPEC.md)。
- **动作**：根据场景库中与本次变更相关的用例，在 `tests/cli_tests.rs` 或 `tests/wasmedge_e2e_tests.rs` 中编写或补充 `test_user_*` / Wasm E2E 用例，与场景库对应。
- **验证**：`RUST_LOG=pi_wasm=debug,info cargo test --test cli_tests -- --nocapture`；环境具备时执行 `wasmedge_e2e_tests`。

### §4 全量测试与验收清单

完成 §1–§3 后须通过以下全量验收。**一键（可选）**：`./scripts/run-integration-tests.sh`

以下为全量验收依据；细节与门禁另见 [INTEGRATION_TEST_SPEC.md](../openspec/specs/guides/testing/INTEGRATION_TEST_SPEC.md) 第 7、9、10 章。

1. **构建与静态检查**：`cargo build --release`、`cargo clippy`、`RUST_LOG=pi_wasm=debug,info cargo test -- --nocapture`
2. **CLI 子命令**：`pi init`、`pi doctor`、`pi config`、`pi session`、`pi plugin`、`pi audit` 可执行且帮助完整
3. **集成测试（含门禁）**：`RUST_LOG=pi_wasm=debug,info cargo test --test '*' -- --nocapture` 通过，含日志门禁与鲁棒性集成测试
4. **E2E**：`RUST_LOG=pi_wasm=debug,info cargo test --test cli_tests -- --nocapture` 通过；WasmEdge 已就绪时 `RUST_LOG=pi_wasm=debug,info cargo test --test wasmedge_e2e_tests -- --nocapture` 通过；须符合 E2E_TEST_SPEC §6
5. **Wasm 真实运行时（若任务涉及插件/Wasm）**：按 INTEGRATION_TEST_SPEC 5.4 执行
6. **对话模式**：`pi chat` 可进入对话；流式输出、多轮上下文、会话切换、4 原语/工具调用与用户确认
7. **插件**：可加载/卸载 pi-mono 风格插件，错误隔离、工具与事件清理正常
8. **跨平台**：若条件具备，在 Windows/macOS/Linux 至少各跑一次 build + test
