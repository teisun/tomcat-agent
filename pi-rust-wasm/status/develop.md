| Owner | Update Time | State | Branch |
| :--- | :--- | :--- | :--- |
| @integration_test | 2026-03-05 22:20 | DONE | develop |

### ✅ DONE (已完成/进行中)
- [✓] **[P0]** 合并 `feature/llm` 至 develop（ort strategy）@2026-03-05
- [✓] **[P0]** 合并后构建与静态检查：`cargo build --release`、`cargo clippy --all-targets` 通过
- [✓] **[P0]** 本波次验收（004）：core/llm（OpenAiProvider、LlmConfig 扩展、类型与 token 统计）已合入
- [ ] **[P0]** 全量单测：`cargo test --all` 现 42 通过、2 失败、1 忽略；2 失败为 `count_tokens_approximate`、`openai_provider_new_succeeds_with_api_key`，因未设置 OPENAI_API_KEY 按宪法要求不通过（非代码缺陷），建议 CI 配置 OPENAI_API_KEY 或由 llm 角色提供无 key 环境下的可接受策略

### 🔌 INTERFACE (接口变更)
- feature/llm 合入：lib 导出 core::llm（LlmProvider、OpenAiProvider、ChatMessage/ChatRequest/ChatResponse、StreamEvent、SessionTokenUsage 等）；LlmConfig 增加 max_concurrent_requests、retry_count、stream_timeout_sec、proxy 等。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 2 个 LLM 单测在无 OPENAI_API_KEY 时失败 | 宪法要求依赖 API key 的用例无 key 时须不通过 | CI 配置 key 或 llm 角色评估无 key 环境策略 |

---

| Owner | Update Time | State | Branch |
| :--- | :--- | :--- | :--- |
| @integration_test | 2025-03-05 14:45 | DONE | develop |

### ✅ DONE (已完成/进行中)
- [✓] **[P0]** 文档与规范：Architecture 渐进式披露（architecture/ 子文档）、examples→guides 重命名、commit-with-status command、Constitution/design 等引用更新 @2025-03-05
- [✓] **[P0]** 合并 `feature/infra` 至 develop（ort strategy）@2025-03-03
- [✓] **[P0]** 合并后全量检查：`cargo build --release`、`cargo clippy`、`cargo test` 通过（32 tests）
- [✓] **[P0]** 本波次验收（001+002）：项目骨架、AppError、配置/日志/跨平台、EventBus 符合 task.md 标准
- [ ] **[P1]** infra：`src/infra/platform.rs` 存在 3 处 dead_code 警告（current_dir、SystemInfo、system_info），建议后续消除

### 🔌 INTERFACE (接口变更)
> 本分支为集成看板分支，不直接引入代码接口变更；当前已合入内容以 feature/infra 的接口为准。
- 无显著变更（汇总自 feature/infra）

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
