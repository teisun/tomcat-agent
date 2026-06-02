| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| @Spike | 2026-06-02 17:25 +0800 | DONE | feature/t2-p0-010-multi-llm-productization | - |

### DONE
- [x] [P0] 认领 `T2-P0-010`，落地 `ModelCatalog` / `LlmResolver` / `AuthStore` 最小闭环
- [x] [P0] 新增 `/model current|list|use`，会话级 `model_override` 持久化与 CLI prompt/横幅显示
- [x] [P0] 扩展 `tomcat init` 为 model-first 向导，按 catalog 推导 provider 凭证
- [x] [P0] 补齐架构 §11 测试锚点，同步 User Stories / E2E 场景库
- [x] [P0] G7 legacy `provider/api_base` 兼容与 init 默认 base 归一化回归修复
- [x] [P0] `cargo fmt`、`cargo clippy -D warnings`、`./scripts/run-integration-tests.sh all` 全绿
- [x] [P1] DeepSeek replay warning 整改：移除 `had_tool_call` wire gate、删除 warning 指纹去重、同 profile `reasoning_content` 默认回放
- [x] [P1] 补齐 `deepseek_non_tool_turn_roundtrip_replays_reasoning_content` 与 post-tool final assistant wire 单测

### INTERFACE
- 新增 `ModelCatalog`、`DefaultLlmResolver`、`AuthStore`；`ChatContext` 主路径经 `resolve_call(LlmScene, override)` 解析 per-call provider。
- 新增 chat 命令 `/model current|list|use`；`/model use <id>` 写入 `SessionEntry.model_override` 并刷新 CLI 当前模型显示。
- `tomcat init` 改为 model-first：先选 `default_model`，再推导 `provider/api/api_key_env` 并写入对应 `<PROVIDER>_API_KEY`。
- `[llm]` 新增可选 `vision_model` / `title_model`；`AgentLoopConfig` 新增 `compaction_llm`。
- DeepSeek continuity：**capture 与 replay 解耦**；snapshot 一律保留，同 family 兼容 `reasoning_content` 默认回放；`had_tool_call` 仅作 transcript 审计 metadata。
- Replay downgrade warning 改为结构化分类（`cross_profile` / `same_profile_incompatible`），不再用进程内指纹缓存压日志。

### BLOCKED
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |

### TEST
- 已跑：`cargo fmt --all`
- 已跑：`cargo clippy --all-targets --all-features -- -D warnings`
- 已跑：`set -a && source .env && set +a && RUST_LOG=tomcat=debug,info ./scripts/run-integration-tests.sh all`（`EXIT_CODE=0`）
- 已跑：`cargo fmt --check`；`cargo test replay_policy_deepseek_v4 --lib`；`cargo test classify_replay_downgrade --lib`；`cargo test transport_messages_deepseek --lib`
- 已跑：`cargo test --test reasoning_continuity_real_llm_tests deepseek_`（3 passed，含 `deepseek_non_tool_turn_roundtrip_replays_reasoning_content`）
- 已核对：`openspec/specs/User_Stories.md`、`openspec/specs/guides/testing/E2E_SCENARIO_LIBRARY.md` 与当前实现一致
