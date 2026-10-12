# 模型 / Provider 绑定不变量

## 1. 不变量（一句话）

**任何 LLM 调用所需的「模型名」与「已绑定 endpoint / 凭证 / 协议的客户端」必须来自同一次 `LlmResolver::resolve`，并以 `ResolvedCall` 成对流动；禁止把裸 model 字符串交给另一路冻结的 `Arc<dyn LlmProvider>`。**

```
                    LlmResolver::resolve(scene, Some(catalog_id))
                                      │
                                      ▼
                         ResolvedCall { sealed }
                    ┌─────────────────┴─────────────────┐
                    │ provider_impl  (客户端)             │
                    │ model          (wire 名)            │
                    │ catalog_id     (catalog 主键)       │
                    └─────────────────┬─────────────────┘
                                      │
                                      ▼
                         AgentLoop::new(binding, ...)
```

## 2. 为什么必须这样

`models.toml` 里每个条目至少有三套彼此独立的字段：

| 字段 | 例子 | 用途 |
|------|------|------|
| `id` | `fcodex/gpt-5.6-sol` | catalog 主键、会话 override、thinking 档位键 |
| `model_name` | `gpt-5.6-sol` | 发给 API 的 wire 名 |
| `provider` + `base_url` + `api_key_env` + `api` | fcodex / fcodex.top / openai-responses | 真正的出口 |

### 2.1 能力与选择也必须分开

`models.toml` 还包含两类**能力/默认值**：`supported_reasoning_levels`，以及 `context_window`（默认）和 `context_window_options`（可选档位）。这些字段与 endpoint 一样，必须随 catalog 条目流动，不能由 UI 猜测。

```text
ModelEntry (能力、默认值)              ModelPrefsStore (个人选择)
├─ supported_reasoning_levels          ├─ reasoning
├─ context_window                      └─ contextWindow
└─ context_window_options                       │
              │                                 │
              └──── resolver / list_models ────┘
                              │
                              v
                 EffectiveModelLimits + wire request
```

选择不是能力：`contextWindow` 只允许取该模型声明的 options；没有 options 时回落到 `context_window`。选择 Reasoning 不会改变 Context。偏好按 catalog `id` 持久化在 `model-thinking.json`，因此 `model_name`（wire 名）变化不会把用户偏好误绑给另一条目录记录。

当前 `core/session/model_thinking.rs::{ModelPrefsFile,ModelPrefs}` 存储 `models` 映射，每个模型值为包含 `reasoning`、可省略的 `contextWindow` 和 `speed` 的对象；缺省 speed 为 Standard。旧 Reasoning 字符串/`__tomcat_context_window__` 保留键并非现行兼容协议，不应再按旧二进制可读来描述。缺失或空文件初始化为空；解析失败时先把原文件保留为 `.corrupt-<timestamp>`，再重建空 store，保留失败则报错。保存先提交候选到磁盘，再更新内存，不把写失败显示成已保存。

Chat 的 `/context`、`/effort` 和 Serve 的 `set_context_window`、`set_thinking_level` 共用同一选择规则。本次只纠正文档，不新增格式兼容、历史迁移或另一个偏好来源。

把「名字」和「出口」拆开传递，就会出现：deepseek endpoint 收到 `fcodex/gpt-5.6-sol` → HTTP 400。

## 3. 事故复盘（2026-07）

配置：

- `llm.default_model = deepseek-v4-flash`
- 会话 `modelOverride = fcodex/gpt-5.6-sol`

旧路径：

1. `ChatContext::from_config` 启动时 `resolve(Main, None)`，只留下 `provider_impl`（deepseek）注入 4 个子 Agent deps；
2. 主 Agent 每回合正确 `resolve(Main, Some(override))`；
3. 子 Agent 派发时 `model = session_model`（catalog id），`llm = 冻结 deepseek`；
4. `openai::effective_model` 因 catalog_model_id 不匹配，原样把 `fcodex/gpt-5.6-sol` 打到 deepseek → 400。

半成品教训：`run_loop` 曾写注释「子 Agent 派发时会拿这个字符串回 catalog 查 provider」，并记录过一次 `claude-opus-4-8` 串台 404；但「回 catalog 查」只做了存 catalog id 这一半，重新 resolve provider 没实现，同类根因再次炸开。

## 4. 现行机制

| 角色 | 做法 |
|------|------|
| 主 Agent | 每回合 `resolve_call` → `AgentLoop::new(main_call, ...)` |
| 子 Agent（plan/code/explorer/verifier） | 派发时 `resolve_subagent_runtime` → `AgentLoop::new(binding, ...)` |
| 插件 HostApi | `llm_for_request` 一律走 `LlmResolver` |
| 测试逃生口 | `ResolvedCall::from_parts_unchecked` 与 `from_provider_and_entry_unchecked`（均 `#[doc(hidden)]`）；前者默认能力，后者保留 catalog 能力与限制，均不得用于生产派发 |

`ResolvedCall` 带私有 `sealed: Sealed`，模块外无法结构体字面量构造。`AgentLoopConfig` 不再持有裸 `model` 字段。 两个 unchecked 工厂是公开但隐藏于文档的测试/适配入口，不等于编译器只允许测试调用；生产调用限制需要遵守 resolver 入口并由源码扫描回归检查。不可将“禁止模块外字面量”夸大为对所有字段修改和工厂调用的静态证明。

## 5. 同行实现调研（证据）

至少查了 6 家同级目录实现：

| 实现 | 结论 | 证据 |
|------|------|------|
| Codex | 子 Agent spawn 强制复制 `turn.model` **与** `turn.provider` | `codex-rs/core/src/tools/handlers/multi_agents_common.rs` `build_agent_shared_config` ≈220–225；测试 `build_agent_spawn_config_uses_turn_context_values`（`multi_agents_tests.rs` ≈4459） |
| Continue | `ILLM` 聚合 model+provider；切模型重建实例 | `ModelService.getSubagentModels` / `createLlmApi` |
| Cline | `ProviderConfig{providerId,modelId}`；model 不在 provider 时 throw | `configured-provider-registry.ts` `createHandlerConfig` |
| OpenCode | 类型强制 `{providerID, modelID}` 二元组 | `Provider.getModel`；`provider.test.ts` invalid model/provider |
| cc-fork-01 | 进程级单 provider，子 Agent inherit 父 resolved model | `utils/model/agent.ts` `getAgentModel` |
| pi | registry 双键 `(provider, modelId)` | `model-registry.ts` `find` |

**共同做法**：下游消费聚合描述符，不接受「裸 model + 旧 client」。Codex 虽在 session client 层部分分离，但子 Agent 路径有显式配对复制 + 测试。

## 6. 什么条件下应当推翻本决策

仅当下列**全部**成立时才可重新讨论「允许 model 与 provider 分头传」：

1. 全仓只剩**一个** LLM endpoint / 凭证（不再有多 provider catalog）；
2. wire 名与 catalog id 永远相等（无 `prefix/model`）；
3. 用类型系统证明不可能出现「名字跟 A、出口跟 B」的组合，并有等价于 Codex `build_agent_spawn_config_uses_turn_context_values` 的回归测试。

否则：缓解（「启动时也解析一次 session override」「给 reviewer 单独配模型」）都是在接受一个本就不该存在的分离能力。

## 7. 相关测试

- `resolve_main_with_session_override_returns_provider_bound_to_that_model`
- `binding_invariant_test`（扫描两个 unchecked 工厂及 `ResolvedCall {`）
- `subagent_uses_provider_matching_session_model_not_startup_default`
- `all_four_subagents_resolve_main_with_session_model`
- `resolve_failure_aborts_with_model_unresolved_and_keeps_plan_file`
- `first_llm_fatal_uses_no_transcript_hint_and_llm_error_stop_reason`
- `h13_plan_reviewer_follows_session_catalog_id_across_providers`
