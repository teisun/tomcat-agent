| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| @Spike | 2026-06-02 14:26 +0800 | DONE | feature/t2-p0-010-multi-llm-productization | - |

### DONE
- [x] [P0] 认领 `T2-P0-010`，落地 `ModelCatalog` / `LlmResolver` / `AuthStore` 最小闭环
- [x] [P0] 新增 `/model current|list|use`，会话级 `model_override` 持久化与 CLI prompt/横幅显示
- [x] [P0] 扩展 `tomcat init` 为 model-first 向导，按 catalog 推导 provider 凭证
- [x] [P0] 补齐架构 §11 测试锚点，同步 User Stories / E2E 场景库
- [x] [P0] G7 legacy `provider/api_base` 兼容与 init 默认 base 归一化回归修复
- [x] [P0] `cargo fmt`、`cargo clippy -D warnings`、`./scripts/run-integration-tests.sh all` 全绿

### INTERFACE
- 新增 `ModelCatalog`、`DefaultLlmResolver`、`AuthStore`；`ChatContext` 主路径经 `resolve_call(LlmScene, override)` 解析 per-call provider。
- 新增 chat 命令 `/model current|list|use`；`/model use <id>` 写入 `SessionEntry.model_override` 并刷新 CLI 当前模型显示。
- `tomcat init` 改为 model-first：先选 `default_model`，再推导 `provider/api/api_key_env` 并写入对应 `<PROVIDER>_API_KEY`。
- `[llm]` 新增可选 `vision_model` / `title_model`；`AgentLoopConfig` 新增 `compaction_llm`。

### BLOCKED
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |

### TEST
- 已跑：`cargo fmt --all`
- 已跑：`cargo clippy --all-targets --all-features -- -D warnings`
- 已跑：`set -a && source .env && set +a && RUST_LOG=tomcat=debug,info ./scripts/run-integration-tests.sh all`（`EXIT_CODE=0`）
- 已核对：`openspec/specs/User_Stories.md`、`openspec/specs/guides/testing/E2E_SCENARIO_LIBRARY.md` 与当前实现一致
