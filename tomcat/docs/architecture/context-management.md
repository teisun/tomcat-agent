本文为 [Architecture](../openspec/specs/Architecture.md) 中「9. Agent Loop 设计」相关的上下文管理专题，总览见主文档。启动期 transcript hydrate 的实现事实源见 [`chat-resume-hydration.md`](./chat-resume-hydration.md)。研究报告：[context-management-deep-dive.md](../reports/context-management-deep-dive.md)。重构建议报告：[context-management-refactoring-proposal.md](../reports/context-management-refactoring-proposal.md)。

---

# 上下文管理技术方案

## 2026-09 Build 马拉松压缩整改

Build 是单个用户请求内连续工具调用的马拉松，不能等待普通 assistant 文本回复才开始准备摘要。

```text
每次请求前（同一 guard 位于 reasoning_loop loop 顶）
  < 50%       → 继续
  50%..100%   → Fits：后台预热当前前缀，记住 covered_end_id
  > 100%      → 预热已就绪：折叠 covered_end_id 以前的前缀，保留之后原始尾巴
             → 未就绪：Step 0 落盘引用 → 运行期工具结果占位符 → 同步全量 Collapse
```

第 2 轮以后，上一条 tool round 结束到下一轮 loop 顶之间不改 `messages`，所以仍是「工具结果已入账、下一次请求尚未构造」的同一个称重点；额外覆盖了每个 loop 的首请求（新 user turn、重试、follow-up、resume）。

`MIDTURN_FOLD_KEEP_RATIO = 0.5` 是刻意固定的工作集策略：摘要后上下文在约 50% 到满额之间摆动，优先保留刚读取的文件和最近工具结果。若观测表明预热常因边界 stale 失效，则只保留 L0–L3 兜底；若 cache 正常但摘要成本成为主要成本，可把保留比降为 0.25。

当前轮的 Step 0 会将单条 ≥10K 字符的结果写入 sidecar，并将消息替换为可重新读取的路径指针；
placeholder wave 仅替换当前进程内的消息，不写 JSONL。hydrate 因此恢复 wave 的原文，再由下一次
请求前的 guard 重新称重和收束。历史 `tool_results_compacted` 记录仍可读取但被忽略，不再覆盖
消息正文。同步 Collapse 从已覆盖消息的 `read`（最近 20 个去重路径）、`edit`、`write` 参数生成
`<recent_files>`，只给路径而不回挂陈旧正文，并提示先查 `git status --short` 和当前 diff。

请求级 ephemeral tail 的物理位置由 provider wire 决定：

```text
Anthropic Messages  : system suffix，cache breakpoint 留在最新持久 tool/user 消息
OpenAI Chat         : leading system message
OpenAI Responses    : top-level instructions
```

tail 变化会有意冷写一次；稳定后必须继续追加可缓存前缀。运行时累计
`prompt_tokens`、`cache_read_tokens`、连续 miss、tail 变化次数及其 miss tokens，并将
cache hit ratio 写入 context metrics 与 `plan.complete`。若同一稳定 prefix 的哈希不变但仍连续 miss，应交由网关排查，而不是继续改写本地 prompt。

Responses tail 进 `instructions` 的依据是 `f2523cc3`，三条 wire 的推广是 `095a36d3`：
稳定 tail 的 DeepSeek 实测优于输入末尾布局，但变化的那一轮会冷写。Build 的无人值守 tail
通常不变，因此维持现状；仅当单一 provider 的 tail 变化率超过 10%，或这些冷写占总 prompt
token 超过 5%，才按 provider profile 评估回退，不作全局位置切换。

## 2026-08 模型额度与运行时尾巴整改

`context_window` 和 `max_output_tokens` 是模型能力，不再由全局上下文配置冒充。模型目录解析出
`EffectiveModelLimits` 后，`ContextState` 使用 `input_budget = context_window - max(model_max_output_tokens, output_reserve_tokens)` 作为压缩水位线的分母。
`[context].context_window_fallback` 与 `output_reserve_tokens` 只服务未知模型和额外安全余量；旧键仍可读取，但新配置不得再把它们当成模型事实上报。

Anthropic 必须发送 `max_tokens`，因此它取模型目录的输出能力（显式请求只能缩小）；OpenAI 未显式限长时不发送
`max_completion_tokens`。thinking budget/effort 只控制推理，不得推导或抬高 wire 输出上限。

请求级 workspace/plan 状态使用 `MessageKind::EphemeralTail`：它不持久化、不开启 user turn，也不切断 reasoning replay。
它的唯一生产者 `EphemeralTailProvider` 只产生文本；三个 adapter 共用提取规则，并共同遵守一条 wire 不变量：
**tail 绝不能作为对话消息出站**。各协议只在 instruction 通道选择不同落点：

```text
Anthropic Messages   → system suffix（不单独标 cache_control）
Chat Completions     → leading system message
OpenAI Responses     → top-level instructions
```

Anthropic 的 system 后缀避免与 `role=user` tool result 合并；它让 D 落在最新持久 tool result 的完整消息末尾。
Chat Completions / Responses 则不能把 tail 留在末尾 user history：下一轮 assistant/tool 会插在它前面，截断自动
prefix cache。稳定 tail 让 durable history 只追加；权限或计划状态变化会使该轮有意冷写一次，随后新的稳定前缀恢复增长。
运行时只记录哈希形式的 `prompt_prefix_fingerprint`，用于定位前缀漂移，绝不记录 prompt 原文。

诊断字段以请求真相命名：`llm_request_resolved.wire_limit_source` 说明上限来自模型目录、未知模型回退还是调用方显式请求；发生 `output_truncated` 时同时写入
`request_max_tokens`、实际 `wire_max_tokens` 与 `thinking_budget`。adaptive thinking 的 `thinking_budget = null` 是正确证据——它发送的是
`effort`，没有 classic budget；绝不把本地 `output_reserve_tokens` 伪装成 wire 上限。

反例与推翻条件：若全历史 replay 在跨模型切换时导致签名校验失败或成本不可接受，可恢复有语义边界的窗口；若两轮指纹一致而 Anthropic 仍无 cache read，应判为网关问题而不是继续改写 prompt。部分正文已产生时的截断 notice 不落盘：它是轻提示，真正无可见输出的失败由持久化 ErrorEntry 覆盖；只有出现需要审计截断正文的产品需求时才改变这一点。

### MCP 工具的渐进式披露

MCP 的完整 schema 是易变且可能很大的运行态目录；它不能随连接/断开进入
`tools` 数组或 system 正文，否则会同时破坏前缀缓存并挤占上下文。稳定前缀只保留
`tool_search` / `tool_describe` / `tool_call`（以及用于扇出聚合的 `tool_run_code`）
这几个固定 builtin 和静态 `connectors` skill 索引。活目录只在调用后作为消息体结果
按 `search → describe → call` 渐进披露，因此连接时序不影响
`prompt_prefix_fingerprint`。设计、边界和测试矩阵见
[`connector-mcp/v2-progressive-disclosure.md`](./connector-mcp/v2-progressive-disclosure.md)。

### Chat Completions 为什么仍保留 `ReplayWindow`

OpenAI Responses 和 Chat Completions 不是同一种协议能力：

```text
Responses:         全历史显式 replay reasoning item
Chat Completions:  旧历史只保留可见对话 ── ReplayWindow ──> 当前轮才带 opaque reasoning
```

Chat Completions 的 `reasoning_content` 兼容字段没有 Responses 的加密 replay 语义和稳定性保证。把每一轮旧 blob
都重发，可能让兼容网关拒绝请求、重复计费，或把已经失效的工具推理带进新回合。因此这里保留窗口：窗口外只保留用户、
正文和工具事实，当前 turn 才带同 profile 的 opaque 续传材料。反例是 Responses：它的 `encrypted_content` 就是为
全历史显式重放设计，所以不套这个窗口。

推翻条件：若某个 Chat Completions provider 文档化并实测支持多轮历史 `reasoning_content`，且全历史重放在同一
模型、跨模型和工具回合中均无 4xx、成本可接受、连续性明显更好，才可针对该 provider 移除窗口；不能因为单次
“看起来能用”而全局删除。

### 2026-08-06 DeepSeek Chat Completions：移动 user tail 截断自动缓存

Chat Completions 没有顶层 `instructions`。旧实现把 runtime tail 原样序列化为末尾 `role=user`；工具循环追加
assistant/tool 后，它的物理位置每轮后移：

```text
旧 wire
rN:     [system] → durable history → runtime tail (user)
rN+1:   [system] → durable history → assistant/tool → runtime tail (user)
                                                     ^ 旧完整前缀在这里断开

正式 wire
rN:     [system + runtime tail] → durable history
rN+1:   [system + same tail]    → durable history → assistant/tool
                                                        ^ 只追加，可继续命中
```

`openai.rs::transport_messages` 在多模态归一化之后剥离 `EphemeralTail`，将非空文本以空行合入**leading**
system；没有 leading system 时才在 durable history 前新建 system。该函数被 `chat` 和 `chat_stream` 共用，
所以两条路径的 wire 一致；`ChatRequest.messages`、transcript、`ReplayWindow` 和 `prompt_cache_key` 均不修改。

真实 DeepSeek `deepseek-v4-pro` Agent-shaped 探针（完整 Agent system / 工具目录、六轮 `config_get`、大确定
tool output、每个模式独立 nonce 防止服务端已有缓存污染）实测：

```text
legacy trailing user tail: 11,648 → 11,648 → 11,648 → 14,080 → 16,512 → 19,072
stable leading system tail:13,184 → 13,184 → 15,616 → 18,176 → 20,608 → 23,168
changing tail at r3:      13,184 → 13,184 →  3,072 → 18,176 → 20,736 → 23,168
```

第六轮的可读缓存从 `19,072 / 25,561 = 74.6%` 升至 `23,168 / 25,644 = 90.3%`；r3 唯一状态变化按预期降至
`3,072`，r4 立即恢复 `18,176` 并在 r6 回到 `23,168`。这是「状态变化可冷写一次，稳定后可恢复」的直接证据，
不是要求每轮数值都上升的脆弱假设（服务端按量化块报告）。

反例：不把 state diff 追加进 transcript，也不把旧权限 / 计划事实留下来与当前约束竞争；不添加 Anthropic
专用 `cache_control` 到 OpenAI-compatible wire。若目标网关拒绝 leading system、稳定 tail 连续实测仍显著
低于旧布局，或其文档化并实测证明末尾 request-only user tail 可获得相同连续命中，才以 provider profile
局部回退，不能全局恢复旧布局。回归由
`tests/prompt_cache_real_llm_tests.rs::deepseek_chat_agent_shape_tail_cache_probe` 与三 adapter 的
`EphemeralTail` wire 单测共同守住。

### 2026-08-04 fcodex Opus 5 M0/M1 实测

M0 对 `fcodex/claude-opus-5` 发出 `max_tokens = 128000` 的真实 Messages 请求并正常得到
`completion_tokens = 8`。因此用户模型目录可声明 `context_window = 1000000` 和
`max_output_tokens = 128000`；这不是从模型名猜出的值。

M1 使用相同工具定义、system、三轮 tool history、adaptive thinking 与 `EphemeralTail`。旧 user-tail
布局的三轮均为 `cache_read=0`、且每轮几乎重写全部输入；这不是两分钟预热，而是 D 位于已合并 user
消息中间块的 wire 失配。改为 system suffix 后，真实 `fcodex/claude-opus-5` 结果为：
`r1 read/write = 0 / 11,122`、`r2 = 11,122 / 4,402`、`r3 = 15,524 / 4,402`。
因此首次写入后，下一请求立即读取；若 `read=0/write>0`，先比较 `tool_hash`、`system_hashes`、
`tail_hashes` 与 selected breakpoints，定位本地 wire 前缀失配，不能以“等待预热”放行。

同日以 `fcodex/gpt-5.6-sol` 走 OpenAI Responses 做了同形状（三轮、tools、xhigh reasoning、
runtime tail）的实测：预热后的三轮 `prompt_tokens = 5655 / 7868 / 10081`，分别读取
`cache_read_tokens = 4608 / 7680 / 7680`。首次运行曾出现 `0 / 4608 / 0`，所以 Responses 的
单轮零 read 也不能当成“本地 prompt 一定坏了”的结论；实际门槛是同一稳定前缀是否在后续请求中出现
cache read，而不是要求每一轮都读到完整历史。

判定条件：在下一请求逐字节复用断点之前的稳定前缀后，若稳定工具/system 断点仍连续两轮
`cache_read_tokens = 0` 且仍有 `cache_write_tokens > 0`，先判定为本地 wire 前缀或断点失配；
携带本段哈希和 selected breakpoints 复核。确认字节前缀相同仍不读回，才向网关排查 usage 语义或
服务端不复用。

### 2026-08-05 Anthropic 缓存断点策略

Anthropic 只会复用某个 `cache_control` 之前的连续字节前缀。因此断点按**出站 wire 消息的物理顺序**分配，不按“用户看见的一轮”
这种逻辑分组分配：

```text
以下是 provider 的缓存构成顺序；不是 JSON 字段显示顺序，也不是对话发生顺序：

tools:
  ... 最后一个工具定义                                             [A]

system:
  稳定 system prompt                                               [B]
  EphemeralTail（无独立 cache_control；稳定时由后面的 D 覆盖）

messages（从旧到新）:
  user:      原始需求
  assistant: tool_use #1
  user:      tool_result #1                                       [C]
  assistant: tool_use #2
  user:      tool_result #2                                       [D]

A = tools 区末尾；B = 最后一个非 runtime system 块；
C = 倒数第二条 wire user 消息末尾；D = 最新持久 wire 消息末尾。
```

- **D 总是完整消息末尾。** fcodex 实测不会复用同一 user 内容数组中、位于 tail 前的中间 block 断点；因此 runtime tail
  不进入 `messages`，D 一律标记最新持久 user/tool-result 的最后 block。
- **C 是滚动读取边界。** 下一次工具调用增长时，服务端仍可读到前一条 `tool_result` 为止的稳定历史。它是 wire 语义：工具循环中常常位于
最后一个逻辑 turn 内部。
- **CompactionSummary 不占 C 的名额。** system 块已经固定前缀；给 summary 再放一个断点通常只多覆盖几 K，而 C 能覆盖持续增长的工具历史。
边界不足四个时自然退化，不造空断点。

`EphemeralTail` 每轮都会重新渲染，因此它是 system 中的变化后缀。它保持不变时，D 覆盖完整的工具历史并持续读取；
它改变时位于整个 `messages` 之前，会让下一请求缓存失效并冷写一次。这是状态正确性优先于命中率的显式取舍。
`phase="wire_request_shape"` 的 `tail_hashes` 用于判断这一失效是否由运行时状态变化造成。

**M6 实测（2026-08-05，`fcodex/claude-opus-5`，真实 8 轮 `tool_use/tool_result` 链）**：
无 tail 的基线从 r2 起连续读回；无论把变化 tail 合并到 user，还是作为连续的独立 user，r2–r5 都是
`read=0` 且重写全部历史。稳定 system tail 的生产布局恢复连续命中：
`r1 read/write = 0 / 11,128`、`r2 = 11,128 / 4,371`、`r3 = 15,499 / 4,371`、`r4 = 19,870 / 4,371`，
最终 r8 为 `37,354 / 4,371`（命中率 89.5%）。这推翻了“tail 前中间 block 的 D 可用”的旧结论。

**参考实现与推翻条件**：Cline 的 `buildClineSystemPrompt`
（`cline/sdk/packages/shared/src/prompt/cline.ts`）把 workspace 与 plan 状态放入 system；Continue CLI 的
`constructSystemMessage`（`continue/extensions/cli/src/systemMessage.ts`）同样把 workspace/git/plan 放进 system，
其 `systemAndToolsStrategy`（`continue/packages/openai-adapters/src/apis/AnthropicCachingStrategies.ts`）缓存 system、
tools 与最近 user 边界。Codex 是反例而非可直接照搬的 Anthropic 方案：它的 Responses client 以稳定
`instructions` 和 thread-scoped `prompt_cache_key` 缓存，并由
`ContextManager::update_world_state`（`codex/codex-rs/core/src/context_manager/history.rs`）只追加 user/developer 状态 diff。
因此角色选择不能脱离 provider wire 语义；只有 fcodex 实测支持“消息中间 block 断点 + 变化 user tail”的连续 cache read，
且 M6 显示其成本不差于 system suffix 时，才应重新评估本方案。

**回归守卫**：CI 单测
`eight_round_marathon_keeps_rolling_c_and_d_with_a_system_tail` 重建八轮 `tool_use → tool_result`，
逐轮断言 C 在上一条稳定 user/tool-result、D 在最新完整消息末尾，tail 是不单独标记的 system suffix，
但稳定时仍由 D 覆盖。它防止 wire 结构回退，但不能伪造服务端 `cache_read` 计费值。需要真实验收时执行
`cargo test --test prompt_cache_real_llm_tests fcodex_opus5_eight_round_marathon_with_system_tail_has_continuous_cache_hits -- --ignored --exact --nocapture`：
从 r2 起必须有 `cache_read_tokens > 0` 且 cache write 小于本轮输入一半；r4–r8 命中率必须大于 80%。
任何这些轮次的 `read=0/write>0` 都是失败，排查 wire 前缀，不得以网关预热放行。

历史改写也遵循同一经济约束。设一次改写损失的原有可读前缀为 `W` tokens、每轮节省为 `S` tokens、后续可复用轮数为 `R`，Anthropic
cache write/read 单价约为 1 : 0.08 时，只有 `R > 12.5 × W / S` 才值得主动改写。故 L0-A（大结果落盘）和 L0-B（历史占位符）
只在成功 Boundary 切换之后运行：切换已使旧前缀失效，L0 只会缩小新前缀；实际溢出时仍由 `aggregate_precheck` 的保命路径处理。
本轮 M4 的三次 `pwd` 没有产生 `>=50K` tool result，不能证明 L0-A 改写最后一个 turn 后仍能读到 C；保守结论是继续让
L0-A 与 L0-B 一起留在 Boundary 后。只有后续带大结果的实测证明改写保住 C 的 cache read，才可把 L0-A 提前到 turn 末尾。

**usage 失效不变量**：`ContextState::apply_boundary()` 在替换历史并重算
`estimate_context_chars` 后已经调用 `invalidate_api_usage()`。因此
`run_layer0_after_boundary()` 只负责继续清理、累计释放量和发事件，**不得再次使 usage
失效**；重复失效不会更安全，只会把已明确的「boundary 改写历史」与后续 L0 清理混为两个
会计动作。例外是 `current_tail_guard` 的保命路径：它可以在**没有**成功 boundary 的情况下
直接执行 `compact_tool_results()`，那次本地改写没有经过 `apply_boundary()`，所以必须在
`current_tail_guard.rs` 自己调用 `invalidate_api_usage()`。

**L0 与 boundary 的绑定及推翻条件**：正常路径把 L0 绑定在成功 boundary 后，是因为
boundary 已经打断缓存前缀，L0 不再引入额外的缓存重写成本。只有引入一种明确「应用
boundary、但保留既有 provider usage 或缓存前缀」的机制（例如可增量拼接且不替换旧历史的
摘要）时，才应推翻此绑定；届时必须重新给 L0 定义独立时机，并用真实 cache read/write
实验验证，而不是把它无条件搬回每轮末尾。

参考实现证据：`cc-fork-01/src/services/api/claude.ts` 的 `markerIndex` 默认取 `messages.length - 1`；Continue 在
`packages/openai-adapters/src/apis/AnthropicCachingStrategies.ts` 对 user 内容块附加 `cache_control`；Codex 将环境状态作为
`<environment_context>`（`codex-rs/protocol/src/protocol.rs`）维护，而非改写旧工具历史。

推翻条件：若同一套 `tool_hash`、`system_hashes`、`message_prefix_hashes` 和 `tail_hashes` 在等待预热后仍连续两次 `cache_read=0`，
停止调整断点并按网关问题处理；若 M4 证明 tail 命中但每轮尾部写成本仍不可接受，才评估把稳定状态转为仅哈希变化时追加的持久消息。

## 2026-08 Plan reminder 规则

计划执行不再是第三种会话模式。请求装配按两个正交事实派生提醒：`AgentMode::Plan` 注入 planner reminder；`PlanRuntime::executing_plan_id().is_some()` 注入 executor reminder。

两段提醒与当前权限状态一起作为请求级 ephemeral tail，附加在已恢复的持久消息之后；它们不进入
`ContextState.messages` 或 JSONL。Anthropic wire 将其转为不单独标记的 system suffix；稳定时由 D 覆盖，
而模式转换与计划生命周期变化会使**下一次**缓存冷写，但不会污染已持久化历史或建立新的 user turn。

## 2026-05 Plan Recover 增补

chat 启动期的 transcript 物理读取、sidecar schema、`latest_plan_event` 快路径、`MAX_PLAN_SCAN` 脱钩、`reverse-chunk` / cold rebuild / kill switch / trace 现在统一收口到 [`chat-resume-hydration.md`](./chat-resume-hydration.md)。

这里仍保留 `init_context_state()` 的**逻辑语义**（fold/filter/boundary/preheat），但不再作为启动期读取算法的事实源。

说人话：`init_context_state()` 现在不一定靠“扫 5000 条尾巴”来找 plan event；启动恢复怎么少读盘，请直接看 [`chat-resume-hydration.md`](./chat-resume-hydration.md)。

## 1. 概述

### 1.1 背景

TASK-17 落地了四层同步防护（Layer 0 截断 → Layer 1 占位符 → Layer 2 LLM 摘要 → Layer 3 强制删除）和 token-aware 滑窗。后续基于 [Claude Code 上下文管理机制](../reports/context-management-refactoring-proposal.md) 的对比分析，升级为 ratio 水位线 + 级联降压模式。

本轮重构将 **LLM 摘要从同步阻塞改为异步预热 + 延迟应用**，核心目标是 **主线程零等待**，避免压缩操作卡住 UI。四层重新定义为：


| 层级      | 名称             | 执行模式               |
| ------- | -------------- | ------------------ |
| Layer 0 | tool_result 清理 | 同步（仅在 Layer 2 成功应用 boundary 后，纯内存操作） |
| Layer 1 | 异步预热           | 异步（后台 Task，主线程不等待） |
| Layer 2 | 检查与应用          | 非阻塞检查 / 仅极端时同步等待   |
| Layer 3 | 物理截断           | 同步（API 报错后兜底）      |


**关键时序定义**：

- **「LLM 回复后」**：指 reasoning loop 的**最终 assistant 回复**——此时当前 user turn 内所有 tool 已执行完毕，LLM 给出了无 tool_calls 的文本回答，reasoning loop 结束。**不是** reasoning loop 内每次中间 LLM 调用之后。
- **「发起下一次 LLM 请求前」**：指**下一个 user turn** 进入时，在构建 `messages` 并调用 LLM 之前。两个时机之间是用户阅读/思考/输入的间隔期，异步预热在此期间后台运行。

核心改进点：

- **Token 计数精度**：从纯字符估算升级为 API Usage 优先 + 字符 fallback
- **异步摘要**：LLM 摘要从同步阻塞改为 Layer 1 异步预热 + Layer 2 延迟应用，LLM 回复后主线程零等待
- **Preheat 状态机（单任务）**：`Preheat` 保证后台同一时间至多一个预热 task，防止竞态和 token 浪费
- **信息保全**：Layer 0 从「截断丢弃」升级为「落盘 + preview 占位符」，大 tool_result 内容不丢失、可按需读回
- **UI 不卡顿**：LLM 回复后仅启动 Layer 1 异步预热或非阻塞地应用已就绪的 boundary；Layer 0 只作为成功 boundary 的后续清理，绝不单独阻塞主线程。仅在发起下一次 LLM 请求前、且 ratio >= 0.98 时才可能同步等待
- **可观测性**：`ContextState` 内嵌 **`live: ContextLiveMetrics`**（瞬时利用率与预热标志）与 **`session_obs: SessionContextObservation`**（会话累计：压缩次数、释放量、工具落盘字符）；`context_metrics_update` 由二者组装，其中 `session_obs` 在 user turn 末写入 `sessions.json`；UI 状态栏反馈压缩进度（见 §10.6）。类型别名 `ContextMetrics` ≡ `ContextLiveMetrics`（见 `context_metrics.rs`）。

### 1.2 设计目标

1. **防溢出**：所有发给 LLM 的 prompt 估算 token 不超过安全水位，消除 context overflow
2. **语义完整**：被压缩的旧消息通过 LLM 结构化摘要保留核心语义（Goal / Constraints / Progress）
3. **信息保全**：超大 tool_result 落盘保全，不截断丢弃，未来可按需读回
4. **主线程零等待**：LLM 回复后绝不阻塞 UI；压缩通过异步预热 + 延迟应用实现
5. **主动降压**：ratio 水位线驱动的分级主动压缩，而非被动等 API 报错
6. **防御性兜底**：Layer 3 物理截断确保极端场景不崩溃
7. **可配置**：`context_window`、`max_output_tokens`、`compaction_model` 等均可在配置中覆盖
8. **可观测**：L0–L3 事件与 `context_metrics_update` 提供分层与累计指标；UI 反馈压缩状态（§10.4 / §10.6）

---

## 2. 术语表


| 术语                     | 说明                                                                                                                                                                                                                        |
| ---------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **user turn**          | 逻辑分组概念：一条 turn-start 消息（`role == User && kind != Steering`，或 `kind == CompactionSummary`）+ 其后所有消息，直到下一个 turn start。上下文管理的最小粒度单位。在内存中通过 `messages: Vec<ChatMessage>` 扁平存储，turn 边界由 `is_turn_start()` 动态判定。 |
| **MessageId**          | Transcript 中每条 `MessageEntry.id`（user/assistant/tool 各一行一条），对应 `ChatMessage.msg_id`（`#[serde(skip)]`）。会话内须**唯一**，且不得包含子串 `::`（与复合 id 分隔符冲突），详见 §5.7。                                                                    |
| **TurnId（复合）**      | `start_id + "::" + end_id`，其中 `start_id` / `end_id` 均为 MessageId。标识一段 turn 的首尾 message。`BranchSummaryEntry.id` 表示「预热 snapshot 首条 turn start 的 `msg_id` + 末条消息的 `msg_id`」，见 §5.7。                                |
| **MessageKind**        | `ChatMessage` 上的枚举字段，取值 `Normal`（默认）/ `Steering`（用户中途注入指令）/ `CompactionSummary`（摘要）/ `EphemeralTail`（本请求临时现场状态）。`EphemeralTail` 不持久化、不建立 turn 边界、不切断 reasoning replay。 |
| **turn start**         | 满足 `(role == User && kind != Steering) \|\| kind == CompactionSummary` 的 `ChatMessage`。`ContextState.turn_count()` 统计 `messages` 中满足条件的消息数。                                                                            |
| **context_window**     | 模型固有的最大上下文长度（输入 + 输出），由模型提供商决定（如 GPT-4o 128K, GPT-5.4 400K）。                                                                                                                                                              |
| **input_budget**       | 输入 token 预算，`context_window - max_output_tokens`。分母，用于计算 ratio。                                                                                                                                                           |
| **ratio**              | 上下文使用率，`estimated_token_count / input_budget`，取值 0.0 ~ 1.0+。驱动多级水位线触发。                                                                                                                                                    |
| **compactable zone**   | `messages` 中可被历史 placeholder 压缩的区间：从开头到 `find_protected_turn_start()` 返回的保护区起始位置之前，排除保护区。保护区来自 `[context].keep_recent_turns`，默认最近 5 个 turn。                                                                                                   |
| **protected zone**     | 最近 `keep_recent_turns` 个 user turns 对应的 `messages` 尾部区间，默认最近 5 个 turn。该区间不参与历史 placeholder 压缩。                                                                                                                       |
| **keep_recent_turns**  | 历史 placeholder 保护区大小（turn 数），来自 `[context].keep_recent_turns`，默认 5。它只影响历史压缩边界；阶段二的 mid-turn current-tail guard 另看 `messages[start_idx..]`。                                                                                                                                       |
| **preview 占位符**        | Layer 0 落盘后替换 tool_result 的短文本，包含路径 + 工具名 + 前 500 chars 预览。                                                                                                                                                               |
| **placeholder**        | Layer 0 替换旧 turn 中 tool_result 的常量文本 `[Previous tool result replaced to save context space]`。                                                                                                                             |
| **CompactionSummary**  | `ChatMessage` 的 `kind == MessageKind::CompactionSummary` 形态，通过 `ChatMessage::compaction_summary(text, marker_id)` 构造。运行时 Layer 1 由 **`Preheat`** 封装 task、3× retry 与 `Idle`/`Running`/`ExhaustedPending`（及重载用的 **`CachedCompleted`**）；成功产物类型为 **`CompactionResult`**（文本 + `covered_*` + **`transcript_compaction_entry_id`**，与 §5.7 中 marker id（整串 `S::E`）一致）。 |
| **预热（Preheat）**        | Layer 1 的异步压缩任务。在 ratio >= 0.5 时启动，克隆 `messages` 后台调用 **解析后的 compaction provider + `compaction_model`** 生成摘要，主线程不等待。快照边界 id 取 **message 级** `covered_start_id` / `covered_end_id`（§5.7）。                                                                |
| **Boundary 切换**        | Layer 2 从 **`preheat`** 取得已完成的 **`CompactionResult`**：先 append `branch_summary_text { forId: marker_id }`，再按 §5.7 `messages.splice(..=end_idx, [summary_msg])` 用 `ChatMessage::compaction_summary(..., marker_id)` 替换前缀；成功后运行 Layer 0。切换后水位从 ~~70% 瞬降至 10~~20%。 |
| **compaction summary** | Layer 1 LLM 对**整个 `messages`** 生成的结构化摘要，一条 `ChatMessage::compaction_summary(text, marker_id)` 替换整批 turns。 |
| **compact boundary**   | 方案 E 的一个逻辑批次有两条 append-only JSONL entry：预热触发时 `BranchSummary { id:X, summary:null, isBoundary:true }` marker；应用时 `BranchSummaryText { forId:X, summary }` 正文。fold 先把正文关联回 marker；marker 无正文时安全 no-op。 |
| **API Usage**          | LLM API 返回的 `usage` 字段（`prompt_tokens` + `completion_tokens`），用于精确 token 计数。                                                                                                                                              |


---

## 3. 核心架构图

### 图一：Token 计数与 Ratio 计算

```
  ════════════════════ Token 计数策略 ════════════════════

  方式 A（优先）：API Usage 精确计数
  ──────────────────────────────────────
  LLM 响应结束时，API 返回本次请求的 token 用量：
      → prompt_tokens  = 180,000  （本次请求的输入 token 数，API 精确值）
      → completion_tokens = 2,000  （本次 LLM 生成的输出 token 数，API 精确值）
      │
      │ prompt_tokens + completion_tokens = 下一轮请求的基线输入量
      │ （因为 LLM 的回复也会成为下一轮的历史消息）
      │
      │ 在 API 响应之后、下一次 LLM 调用之前，
      │ 可能追加了新消息（如 tool result），这部分没有精确 token 数，
      │ 只能用字符数 / 4 估算：
      │   post_usage_appended_chars = 12,000 chars → ~3,000 tokens
      ▼
  estimated_token_count = (prompt_tokens + completion_tokens)
                        + post_usage_appended_chars / 4
                        = (180,000 + 2,000) + 3,000
                        = 185,000

  注意：绝大部分 token 计数来自 API 返回的精确值（prompt_tokens + completion_tokens），
  字符数 / 4 仅用于估算「最近一次 API 响应之后新追加的消息」这一小段增量。

  方式 B（fallback）：字符启发式
  ──────────────────────────────────────
  首轮无 usage / Boundary 切换后旧 usage 失效时
      → estimated_token_count = estimate_context_chars / 4
  此模式仅短暂使用，等下一次 LLM 响应即可切回方式 A。


  ════════════════════ Ratio 与水位线 ════════════════════

  input_budget = context_window - max_output_tokens
               = 400,000 - 128,000 = 272,000 tokens（GPT-5.4）

  ratio = estimated_token_count / input_budget

    0%          50%     70%    85%           98%  100%    API Error
    ├───────────┼───────┼──────┼─────────────┼───┤         │
    │  正常区    │ L1    │ L2   │L1+L2        │L2 │         ▼
    │  无压缩    │预热   │请求前│回复后检查+   │请求前:    L3 物理截断
    │           │async  │检查  │请求前检查+   │同上+      │ 目标<0.50
    │           │       │      │可能启动新预热│sync wait  │

  注：Layer 0 仅在成功 Boundary 切换后运行；实际溢出时由请求前保命路径执行等价清理。
      「LLM 回复后」= user turn 结束（reasoning loop 最终回复，所有 tool 已执行）。
      「发起 LLM 请求前」= 下一个 user turn 进入时。
      100% 水位本身不触发 Layer 3；Layer 3 仅在 API 明确返回 Context Overflow 错误时触发。
      LLM 回复后 ratio 允许暂时超过 0.98 甚至 >1.0（input_budget 已扣除 max_output_tokens，有余量）。
      仅在发起下一次 LLM 请求前，ratio >= 0.98 时才可能同步等待。
```

### 图二：四层防护流程

```
  Layer 2 在② / ⑤ / mid-turn发现已完成的预热摘要
  apply_boundary 成功：历史前缀已替换为摘要
      │
      ▼
  ┌─ Layer 0（作为成功 Boundary 的后续步骤，同步）───────────┐
  │  A. 单条 tool_result >= 50K chars？                    │
  │     → 落盘 + 500 chars preview 占位符                  │
  │  B. compactable zone (messages[..protected_start]) 中  │
  │     tool_result >= 10K chars？                         │
  │     → 占位符替换（不落盘）                             │
  │  C. 作废被驱逐 tool_call 对应的 read stamp              │
  │  D. 重新估算 tokens 用量和水位，非零时发释放事件          │
  └────────────────────────────────────────────────────────┘
      │  （未切换时整段不运行，历史保持字节一致）
      ▼ 计算新 ratio
      │
      ratio >= 0.50 且无进行中的异步任务？
      │  ──Yes──► 触发 Layer 1（异步，不等待）
      │
      ▼
  ┌─ Layer 1（异步预热，主线程不等待）─────────────────────┐
  │  1. 克隆当前 messages: Vec<ChatMessage>                 │
  │  2. 启动后台 Task：                                    │
  │     → 按模板压缩整个 messages（记录首尾 msg_id）        │
  │     → 调用 compaction_model，限制 <= 10K tokens         │
  │     → 写入 transcript: type=branch_summary, boundary=false   │
  │        （插入在 id==covered_end_id 的 message 行**之后**，§5.7）│
  │  3. 产物 → CompactionResult（由 Preheat 持有至 Layer 2 消费）   │
  │  单例：后台只允许一个压缩任务                           │
  └────────────────────────────────────────────────────────┘
      │
      ▼ 主线程继续（不等待）
      │
      ratio >= 0.85？
      │  ──Yes──► Layer 2 - LLM 回复后检查（非阻塞）
      │            preheat 已有可应用结果？
      │              → Yes: 立即 Boundary 切换
      │              → No:  跳过，不等待
      │
      ▼ 当前 user turn 处理完毕
      ·
      · （用户阅读回复、思考、输入下一条消息）
      · （异步预热在此期间后台运行）
      ·
      ▼
  ┌─ 下一个 user turn 发起 LLM 请求前 ─────────────────────┐
  │  ratio >= 0.70？                                       │
  │    → try_restart_if_pending；preheat 已有结果则 Boundary 切换 │
  │                                                        │
  │  ratio >= 0.98？                                       │
  │    → Layer 2 - 发请求前检查：                          │
  │      已有结果？→ 直接 Boundary 切换                        │
  │      未完成？→ **化异步为同步**（await_result）           │
  │        阻塞等待摘要完成，再 Boundary 切换               │
  │        （阻塞的是推理启动，UI 已完成渲染）              │
  └────────────────────────────────────────────────────────┘
      │
      ▼ 发起 LLM 请求
      ·
      · （若 API 返回 Context Overflow 错误）
      ·
      ▼
  ┌─ Layer 3（物理截断，防御性兜底）───────────────────────┐
  │  从最旧 summary/turn 起逐条删除，直到 ratio < 0.50     │
  │  （几乎不可达的安全网）                                │
  └────────────────────────────────────────────────────────┘
```

### 图三：滑动窗口与保护区

```
  messages: Vec<ChatMessage> (内存中维护，扁平存储)：

  keep_recent_turns = 5 个 turn（默认，可配置）:
  [msg_0] [msg_1] ... [msg_p-1] │ [msg_p] ... [msg_n-1]
  ◄──── compactable zone ──────►│◄──── protected zone (最近 5 turns) ──►
  （历史 placeholder 压缩适用此分区；mid-turn current-tail guard 另看 messages[start_idx..]）
  p = find_protected_turn_start() 返回的索引

  Layer 0 占位符替换：作用于 messages[..p] 中 role==Tool 且内容 >= 10K 的消息
  Layer 1 异步预热：摘要覆盖**整个 messages**（记录首尾 msg_id `S`/`E`，§5.7），不区分保护区
  Layer 2 Boundary 切换：用 ChatMessage::compaction_summary(...) 替换 messages[..=end_idx]，保留之后新增的消息
```

### 图四：异步预热与 Boundary 切换演进

```
  ════════════════════ 初始状态 ════════════════════

  [msg_0][msg_1]...[msg_9] [msg_10]...[msg_n-1]
  ◄──────────── 整个 messages: Vec<ChatMessage> ──────────────────►

  ════════════════════ ratio >= 0.50 → Layer 1 异步预热 ════════════

  前台：确认 covered_end == transcript 尾 → append marker
        { type: branch_summary, summary: null, is_boundary: true, id: S::E }
  后台 Task：克隆整个 messages → 调用 compaction_model
  → 生成 summary_A（Goal/Constraints/Progress...）
  → 原子覆盖写 pending preheat cache（不写 transcript）
  → CompactionResult { summary_text: summary_A, covered: msg_0..msg_n-1, ... }（由 Preheat 暂存）

  主线程不等待，对话正常继续。

  ════════════════════ ratio >= 0.70 → 发起 LLM 请求前检查 ════════════

  preheat 已有可应用结果？
    → Yes: 执行 Boundary 切换（非阻塞）
           append branch_summary_text { forId: S::E, summary: summary_A } 后切换内存
    → No:  跳过，不阻塞

  ════════════════════ ratio >= 0.85 → LLM 回复后检查（⑤，非阻塞）════════════

  preheat 已有可应用结果？
    → Yes: 立即执行 Boundary 切换
           append branch_summary_text { forId: S::E, summary: summary_A } 后切换内存

  [summary_A] [msg_new_1]...[msg_new_k]
  ◄─ 1 条摘要 ──► ◄── Layer 1 快照后新增的消息 ──►

  ratio 降回 ~10-20%（常规场景；若快照后有大量新增 turns，降幅可能不到位，
  会触发新一轮 Layer 1 预热）

  ════════════════════ ratio >= 0.98 → 发起 LLM 请求前强制检查（②）════════════

  try_restart_if_pending；已有结果？→ 直接 Boundary 切换
  未完成？→ 化异步为同步（await_result），再 Boundary 切换

  ════════════════════ 已有旧 summary 时 ════════════════════

  若后续 ratio 再次达 0.50，summary_A 与新消息均在 messages 中：
  Layer 1 使用 UPDATE 模式合并旧 summary（参考 UPDATE_SUMMARIZATION_PROMPT）
```

---

## 4. 预算计算与水位线

### 4.1 Token 计数策略

精确的 token 计数是所有压缩决策的基础。采用 **API Usage 优先 + 字符 fallback** 双模式：

```
fn estimated_token_count(state: &ContextState) -> usize:
    if let Some(usage) = state.last_api_usage:
        let base = usage.prompt_tokens + usage.completion_tokens
        let incremental = state.post_usage_appended_chars / CHARS_PER_TOKEN_ESTIMATE
        base + incremental
    else:
        state.estimate_context_chars / CHARS_PER_TOKEN_ESTIMATE

fn usage_ratio(state: &ContextState) -> f64:
    estimated_token_count(state) / state.context_budget_tokens
```

- `**last_api_usage**`：每次 LLM 响应结束后，从 `StreamEvent::Usage` 更新。`TokenUsage.prompt_tokens` 的跨 provider
  契约是“本次请求的输入总量”：OpenAI 的 `prompt_tokens` 已是总量；Anthropic 在 wire 边界归一为
  `input_tokens + cache_read_input_tokens + cache_creation_input_tokens`。`cache_*` 只是该总量的计费分档，未来计费不可用总量直接乘单价。
- `**post_usage_appended_chars**`：自最近一次 API usage 后、尚未被 usage 覆盖的新用户输入和 tool result 的字符数，以
  `字符数 / 4` 作为增量估算。assistant 正文只增加 `estimate_context_chars`，**不**增加此计数器：同一轮 usage 的
  `completion_tokens` 已经精确包含它，再加一次会使下一轮水位虚高。
- **compact 后**：`last_api_usage` 失效（上下文已变），清零回退到字符 fallback，等下次 API 响应刷新

> **关于 `estimate_context_chars` 的度量单位**：Rust 的 `String::len()` 返回 UTF-8 字节数而非 Unicode 字符数。`CHARS_PER_TOKEN_ESTIMATE = 4` 对英文内容（1 byte ≈ 1 char）较为准确；对中文内容（3 bytes/char，约 1.5 token/char），4 bytes/token 的估算会偏保守（低估 token 数），可能导致压缩触发略晚。**API Usage 优先模式下此偏差被消除**，字符 fallback 仅在首轮和 compact 后短暂使用。

**当前决策：保持 `/4` 不变。** 真实会话曾观测到 1.3–3.5 字符/token 的范围，但修改常数会同时改变 fallback、
`context_budget_chars` 与既有水位测试基线。本次只用 `phase="context_estimate_check"` 记录每次请求前估算与该请求
provider usage 的误差；收集到按语言、provider、请求阶段分组的稳定分布后再决定。推翻当前决策的条件是：fallback
或 Boundary 后的请求连续显著低估，且该误差已导致可复现的压缩延迟或 context overflow；届时应替换为经过实测校准的
估算策略，而不是凭单个会话调整常数。

### 4.2 Ratio 水位线

`ratio = estimated_token_count / input_budget`，其中 `input_budget = context_window - max_output_tokens`。

分母是输入 token 预算（已扣除输出预留），ratio 衡量的是输入空间的使用率，不会挤占输出空间。


| ratio 档位         | 触发层       | 检查时机              | 动作                                                                      |
| ---------------- | --------- | ----------------- | ----------------------------------------------------------------------- |
| Boundary 切换成功后      | Layer 0   | ② / ⑤ 的成功 apply 后  | 清理摘要之后的 tool_result；未切换时保持历史字节级不变                                        |
| `>= 0.50`        | Layer 1   | LLM 回复后（⑤）        | `try_restart_if_pending` → 通过统一 `compaction_provider()` 入口异步预热 `preheat.try_start`（若无进行中的任务），主线程不等待                                                   |
| `>= 0.70`        | Layer 2   | **发起 LLM 请求前（②）** | `try_restart_if_pending` → 检查 `preheat` 结果，完成则 Boundary 切换（非阻塞）                               |
| `>= 0.85`        | Layer 1+2 | LLM 回复后（⑤）        | `try_restart_if_pending` → 先 `poll_result`：**已有结果则立即 Boundary 切换**（非阻塞）；再判断是否需要新一轮 `try_start`      |
| `>= 0.98`        | Layer 2   | **发起 LLM 请求前（②）** | `try_restart_if_pending` → **已有结果则直接 Boundary 切换**（非阻塞）；仅**未完成**时 `await_result` 化异步为同步阻塞等待 |
| Context Overflow | Layer 3   | API 返回错误后（③内）     | 物理截断至 ratio < 0.50                                                      |


**设计原则**：

- **LLM 回复后绝不阻塞主线程**，即使 ratio 暂时超过 0.98 甚至 >1.0 也只做 L1 异步预热，或非阻塞地应用已完成的 Layer 2；只有 boundary 已成功应用时才紧随其后执行 L0，不卡 UI
- 因为 `input_budget = context_window - max_output_tokens`，LLM 回复完成时实际还有 `max_output_tokens` 的空间余量，ratio 超过 1.0 不代表立即 Context Overflow
- **仅在发起下一次 LLM 请求前**才可能阻塞（L2 化异步为同步），此时 UI 已完成当前轮的渲染，阻塞的是推理启动而非 UI 交互

**触发总表**（含 Layer 0 细节）：


| 触发条件                                                      | 层级        | 时机           | 动作                                   |
| --------------------------------------------------------- | --------- | ------------ | ------------------------------------ |
| 成功应用 boundary 后，单条 tool_result >= 50K chars          | Layer 0   | ② / ⑤ / 中途守卫 | 落盘 + 500 chars preview 占位符           |
| 成功应用 boundary 后，compactable zone 中 tool_result >= 10K chars | Layer 0 | ② / ⑤ / 中途守卫 | 占位符替换（不落盘）                           |
| ratio >= 0.50 且 preheat 可启动                                   | Layer 1   | ⑤            | `try_restart_if_pending` → `preheat.try_start`（后台 Task，内 3× retry）                   |
| ratio >= 0.70                                             | Layer 2   | ② 发起 LLM 请求前 | `try_restart_if_pending` → `poll_result`/`apply`，完成则 Boundary 切换 |
| ratio >= 0.85                                             | Layer 1+2 | ⑤ LLM 回复后    | `try_restart_if_pending` → 非阻塞 `poll_result` + 切换 + 可能 `try_start`                 |
| ratio >= 0.98                                             | Layer 2   | ② 发起 LLM 请求前 | `try_restart_if_pending` → 已完成→切换；未完成→`await_result` 同步等待                      |
| API 返回 Context Overflow                                   | Layer 3   | ③ 内          | 物理截断至 ratio < 0.50                   |


### 4.3 配置项


| 配置项                                  | 类型       | 默认值         | 说明                                                                                                                                |
| ------------------------------------ | -------- | ----------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `context_window`                     | `usize`  | `400_000`   | 默认对齐 **GPT-5.4**（400K）；其他模型请在配置中覆盖                                                                                                |
| `max_output_tokens`                  | `usize`  | `128_000`   | 默认对齐 **GPT-5.4** 单轮最大输出；对齐 API 的 `max_tokens`                                                                                     |
| `layer0_single_result_max_chars`     | `usize`  | `50_000`    | Layer 0 触发条件 A：单条 tool_result 超过此值则落盘 + preview 占位符                                                                               |
| `layer0_placeholder_threshold_chars` | `usize`  | `10_000`    | Layer 0 触发条件 B：compactable zone 中 tool_result 超过此值则占位符替换                                                                          |
| `keep_recent_turns`                  | `usize`  | `5`         | 历史 placeholder 压缩的保护区 turn 数；最近几轮先不动                                                                                               |
| `current_tail_compactable_min_chars` | `usize`  | `1`         | 阶段二 mid-turn current-tail guard 的候选最小字符数                                                                                                |
| `current_tail_single_result_max_chars` | `usize` | `10_000`    | 阶段二 mid-turn current-tail guard 的大结果落盘阈值                                                                                                |
| `compaction_model`                   | `String` | `"gpt-5.2"` | Compaction 摘要专用模型 ID（与主对话 `model` 可相同或不同）                                                                                         |
| `compaction_max_tokens`              | `usize`  | `10_000`    | Layer 1 异步预热生成摘要的 token 上限（预留）。当前**不设 API `max_tokens` 硬限制**以保证摘要语义完整性；仅在 prompt 中软引导 LLM 控制在 ~8K tokens 篇幅。未来若摘要频繁超标，可启用 API 硬限制 |


> 配置位于 `tomcat.config.toml` 的 `[context]` 节，或通过 `PrimitiveConfig` 结构体注入。
>
> 阶段二新增的 mid-turn current-tail guard 发生在每次工具轮结束、下一次 `llm.chat_stream(...)` 之前。它优先复用历史摘要与历史 placeholder，再看 `messages[start_idx..]` 的 current tail；不要把它和下一 user turn 的 preheat / timing② 混成一件事。

### 4.4 典型值


| 模型                | context_window | max_output_tokens | input_budget | ratio=0.50 时已用 |
| ----------------- | -------------- | ----------------- | ------------ | -------------- |
| GPT-4o            | 128,000        | 16,384            | 111,616      | 55,808         |
| GPT-5.4           | 400,000        | 128,000           | 272,000      | 136,000        |
| Claude 3.5 Sonnet | 200,000        | 8,192             | 191,808      | 95,904         |
| DeepSeek-V3       | 64,000         | 8,192             | 55,808       | 27,904         |


### 4.5 与旧方案对比

旧方案（TASK-17）使用 `contextBudgetChars = (context_window - max_output_tokens) × 4 × 0.75`，额外乘 0.75 安全系数用于补偿字符→token 估算误差。新方案有了精确 token 计数后，0.75 系数不再需要——ratio 水位线本身提供分级保护，且 `is_over_budget()` 改为基于 token 维度判断。

---

## 5. 初始化与动态维护

### 5.1 会话启动时初始化

> 这里的伪代码描述的是“恢复后如何 fold/filter 出上下文”的逻辑语义，不再描述 transcript 的物理读取方式。真实启动路径的 `Full / Tail / Auto`、sidecar、count-based 锚点匹配、cold rebuild 等细节见 [`chat-resume-hydration.md`](./chat-resume-hydration.md)。

```
fn init_context_state(session: &Session, config: &ContextConfig) -> ContextState:
    msgs = fold_entries_to_messages(session.transcript_path)

    # 优先取当天所有消息
    today_msgs = msgs.filter(|m| m.timestamp.date() == today())

    # turn 数不足 10 则向前补全（确保短会话或跨午夜场景仍有足够上下文）
    if count_turn_starts(&today_msgs) < 10:
        extra = msgs.before(today_msgs.first())
                     .rev()
                     .take_until(count_turn_starts >= 10)
        today_msgs = extra.rev() + today_msgs

    let input_budget = config.context_window - config.max_output_tokens
    estimate = sum(today_msgs.map(|m| estimate_msg_chars(m)))

    return ContextState {
        messages: today_msgs,
        estimate_context_chars: estimate,
        context_budget_chars: input_budget * CHARS_PER_TOKEN_ESTIMATE,  # fallback 用
        context_budget_tokens: input_budget,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: ...,  # 当前会话 JSONL
        preheat: Preheat::default(),  # Layer 1 异步预热状态机
        session_obs: ...,  # 从 SessionEntry 恢复累计；新建即默认 0
        live: ContextLiveMetrics::default(),
    }
```

> **边界情况说明**：
>
> - **跨午夜会话**：用户 23:55 开始对话，重启后 `today()` 返回新日期。当天消息为空时，向前补全最近 10 个 turn 覆盖前一天的对话，不影响正确性。
> - **长期不活跃**：transcript 最后活跃在数天前，补全的消息为旧内容。上下文可能已不相关，但不影响正确性——后续新对话产生后旧消息会自然进入 compactable zone 被压缩。
> - 此策略优先保证**不丢失近期上下文**；上下文"相关性"由 Layer 1 摘要在运行过程中自然优化。

### 5.2 `Preheat` 与 `CompactionResult`

**`CompactionResult`**（预热成功时的产物，与 Layer 2 应用、`ChatMessage::compaction_summary(...)` 展平形态对应）：

实现中另有 `transcript_compaction_entry_id`、`estimated_*`、`preheat_elapsed_ms` 等字段；与 §5.7 对齐的**核心语义**如下：

```
struct CompactionResult {  // 规范语义（字段名以实现为准）
    summary_text: String,
    covered_start_id: String,   // = 预热 snapshot 首条 turn-start 消息的 msg_id（MessageId）
    covered_end_id: String,     // = 预热 snapshot 最后一条消息的 msg_id（MessageId）
    covered_count: usize,
    transcript_compaction_entry_id: Option<String>,  // 须与 BranchSummaryEntry.id 整串一致，即 S::E
    // …估算与耗时等
}
```

**`Preheat`**：封装 Layer 1 的完整状态机（内部 `Idle` / `Running` / `ExhaustedPending`，对调用方不可见）。`ContextState` 字段为 **`preheat: Preheat`**（非 `Option`）。

**对外方法**：

- `try_start(...)`：ratio 等条件满足且当前可启动时 spawn **唯一**后台 task；`Running` 或已有可应用的完成结果时不再重复启动。
- `try_restart_if_pending(...)`：在 **`ExhaustedPending`**（3× retry 耗尽）时，若条件仍满足则重新启动；与 §6.6、步骤 ⑤/② 双点调用配合。
- `poll_result()` / `await_result()`：供 Layer 2 非阻塞探测或发请求前同步等待。
- `abort()`：任意状态 → `Idle`，取消 task、清理 pending。

**生命周期**：Session 销毁或用户退出时应调用 **`preheat.abort()`**，等价于取消 `JoinHandle` 并复位状态；勿依赖仅 drop handle 来取消 task。

### 5.3 动态更新

追加消息分成两条路径，避免把 assistant 正文计两次：

```
fn on_message_appended(state: &mut ContextState, chars: usize):
    // 用户输入、tool result、nudge：下一次 usage 尚未覆盖，需增量估算。
    state.estimate_context_chars += chars
    state.post_usage_appended_chars += chars

fn on_assistant_message_appended(state: &mut ContextState, chars: usize):
    // assistant 的 completion_tokens 已在同一轮 API usage 中，不能再叠加。
    state.estimate_context_chars += chars

fn update_api_usage(state: &mut ContextState, prompt_tokens: u32, completion_tokens: u32):
    state.last_api_usage = Some(ApiUsage { prompt_tokens, completion_tokens })
    state.post_usage_appended_chars = 0

fn invalidate_api_usage(state: &mut ContextState):
    state.last_api_usage = None
    state.post_usage_appended_chars = 0
```

> `invalidate_api_usage` 在 Boundary 切换后调用——上下文已变，旧 usage 不再有效。

### 5.4 system prompt 纳入估算

`estimateContextChars` 应包含 system prompt 的字符数。system prompt 在会话期间通常不变，初始化时计算一次即可：

```
estimate = system_prompt.len() + sum(today_msgs.map(|m| estimate_msg_chars(m)))
```

> 若 system prompt 较短（< 5K chars），水位线已足够覆盖。但为准确性，仍建议显式计入。
> 若 system prompt 在会话中可能变化（如工具动态注册/卸载导致工具描述段变化），应在每轮 ② 构建 `messages` 时重新计算 system prompt 字符数并更新 `estimateContextChars`。

### 5.5 Session 重载与 Compact Boundary

从 transcript JSONL 加载消息时，方案 E 将「切口」和「摘要正文」拆为两条 append-only entry：

```
entry 1~8:  原始消息（将被覆盖）
entry 9:    branch_summary { id:X, summary:null, isBoundary:true }  ← 触发预热时的切口
entry 10~11: 活尾的新消息
entry 12:   branch_summary_text { forId:X, summary:"..." }          ← 应用时到达的正文

fold 第一遍: 收集 X → 摘要正文
fold 第二遍: 读到 entry 9 才清掉 1~8、放入 compaction_summary(X)
             entry 10~11 保留；entry 12 本身不生成消息
```

因此 marker 没有正文时是 no-op，绝不能清掉原消息。正文在物理文件尾部，也绝不能在它自身的位置触发 clear。已应用后的 `<session>.preheat.jsonl` 缓存因 transcript 已有正文而自动失效；只有「marker 存在 + 无正文 + covered end 仍在当前消息」时才恢复 pending result。

**被压缩的 user turn 是否仍留在 transcript JSONL 中？**

采用 **所有 compaction 行仅追加** 约定：

- **保留**：原先写入的 `Message` 行（user / assistant / tool）**不删除、不改写**，仍在 `.jsonl` 中，便于审计、回放与调试。
- **BranchSummary（transcript 行）**：触发预热时，`covered_end_id` 仍是尾条，前台追加 `type: branch_summary` marker（`id = S::E`、`summary:null`、`isBoundary:true`）。应用时前台再追加关联 `type: branch_summary_text` 正文（`forId = S::E`）；不再原地改写 marker 或删除行。旧的内联 `branch_summary` 与 `isBoundary=false` 记录仍可读取，以兼容历史 transcript。
- **构建 LLM 上下文**：`messages` / `build_context_from_state`（直接返回 `state.messages.clone()`）在内存中按 Compaction 元数据 **折叠**——已摘要区间只表现为一条 `ChatMessage::compaction_summary`，**不把同一区间的原始消息再次拼进 prompt**（避免双倍 token）。

若未来需要「物理瘦身」大文件，可作为独立运维能力（压缩归档副本），**不**作为默认行为。

### 5.6 `messages` 与消息结构的关系

#### 统一的消息表示

重构后消息表示从四层简化为两层（旧：transcript JSONL → TurnEntry → AgentMessage → ChatMessage）：

```
  ┌─ transcript JSONL ─┐    ┌── ChatMessage ───────────────────────────────┐
  │ serde_json::Value   │    │ 统一类型：LLM wire + 内部工作集 + 上下文管理  │
  │ (磁盘持久化格式)    │    │ role + content + tool_calls                  │
  └─────────┬──────────┘    │ + msg_id / kind / timestamp (#[serde(skip)]) │
            │                └─────────────────────────────────────────────┘
            │ fold_entries_to_messages
            │ （Compaction 区间折叠为 ChatMessage::compaction_summary）
            └──────────────────────────►│
                                        ▼
                              messages: Vec<ChatMessage>
                              ContextState 上的扁平消息列表

  ④ LLM / run 结束后：
       new_messages = messages[start_idx..]
       → 逐条 messages.push(msg) 追加到 ContextState.messages
       → serde_json 写入 JSONL

  落盘：ChatMessage ──serde_json──► JSONL 行追加（msg_id/kind/timestamp 被 skip）
```

- `**ChatMessage**`：统一的消息类型（`role` + `content` + `tool_calls`），由 `src/core/llm/types.rs` 定义。已有的 `Steering`（用户中途注入指令）和 `CompactionSummary`（摘要）通过 `kind: MessageKind` 字段（`#[serde(skip)]`）区分，无需单独类型。
- `**ChatMessage.msg_id**`：对应 transcript 行 id，`#[serde(skip)]`。
- `**ChatMessage.timestamp**`：ISO 时间字符串，`#[serde(skip)]`。
- **构造器**：`ChatMessage::steering(text)` 构造 steering 消息；`ChatMessage::compaction_summary(text)` 构造摘要消息。
- **Turn 边界判定**：`is_turn_start()` → `(role == User && kind != Steering) || kind == CompactionSummary`，动态识别 turn 起始，无需分组数据结构。

#### `messages` 的定位

`messages` 是 **上下文管理模块的扁平内存视图**，直接存储 `Vec<ChatMessage>`，无分组层：

```
  messages: Vec<ChatMessage>
      │
      ├─ ChatMessage { role: User, kind: Normal, msg_id: "m1", ... }       // turn start
      ├─ ChatMessage { role: Assistant, kind: Normal, msg_id: "m2", ... }
      ├─ ChatMessage { role: Tool, kind: Normal, msg_id: "m3", ... }
      ├─ ChatMessage { role: User, kind: Normal, msg_id: "m4", ... }       // turn start
      └─ ...
      # 或含摘要：
      ├─ ChatMessage { role: User, kind: CompactionSummary, msg_id: ..., ... }  // turn start（摘要）
      └─ ...
```

- `ContextState.turn_count()` 统计 `messages` 中满足 `is_turn_start()` 条件的消息数。
- `find_protected_turn_start()` 从尾部倒数 `keep_recent_turns` 个 turn start，返回保护区起始索引。

#### 发给 LLM 的完整链路

```
  ┌─────────────────────────────────────────────────────────────────────────┐
  │ ① 会话初始化（用户首次输入前，一次性）                                    │
  │                                                                         │
  │  transcript JSONL ──[BufReader 逐行]──► fold_entries_to_messages        │
  │       │                                      │                          │
  │       │  识别 Compaction entry + boundary     │ 折叠已摘要区间            │
  │       ▼                                      ▼                          │
  │  messages: [ChatMessage::compaction_summary?, ChatMessage, ...]          │
  │  estimateContextChars = system_prompt.len() + Σ msg_chars               │
  └─────────────────────────────────────────────────────────────────────────┘
                          │
                          ▼
  ┌─────────────────────────────────────────────────────────────────────────┐
  │ ② 每轮对话进入前（用户按下回车）                                          │
  │                                                                         │
  │  ◆ 发起 LLM 请求前检查（详见下方 ⑤→② 循环）：                            │
  │    preheat.try_restart_if_pending(...)（② 补偿 ExhaustedPending）        │
  │                                                                         │
  │  build_context_from_state(state) → state.messages.clone()               │
  │       + 注入 system prompt                                              │
  │       + 追加本轮 ChatMessage::user(...)                                 │
  │       ──► initial_messages: Vec<ChatMessage>                            │
  │                                                                         │
  │  AgentLoop::run(initial_messages)                                       │
  └─────────────────────────────────────────────────────────────────────────┘
                          │
                          ▼
  ┌─────────────────────────────────────────────────────────────────────────┐
  │ ③ reasoning loop 内（LLM ↔ 工具循环，可能多轮）                           │
  │                                                                         │
  │  messages: &mut Vec<ChatMessage>    ← AgentLoop 工作集                   │
  │       │                                                                 │
  │       │  ChatMessage 已是 LLM wire 格式，直接作为 ChatRequest.messages   │
  │       ▼                                                                 │
  │  Vec<ChatMessage> ──► ChatRequest.messages ──► llm.chat_stream          │
  │       │                                                                 │
  │       │  LLM 返回 assistant + tool_calls                                │
  │       │  工具执行 → tool result                                         │
  │       ▼                                                                 │
  │  messages.push(ChatMessage { role: Tool, .. })                          │
  │  estimateContextChars += result.len()       ← 实时更新                  │
  │  update_api_usage(usage)                    ← 从 StreamEvent 更新       │
  │                                                                         │
  │  reasoning loop 内 messages 自由增长，不做压缩。                          │
  │  若 API 返回 Context Overflow → 触发 Layer 3 物理截断（见 §6.4）         │
  │  最终 LLM 回复（无 tool_calls）→ 退出 reasoning loop                    │
  └─────────────────────────────────────────────────────────────────────────┘
                          │
                          ▼
  ┌─────────────────────────────────────────────────────────────────────────┐
  │ ④ user turn 完成：追加 + 持久化                                          │
  │                                                                         │
  │  new_messages = messages[start_idx..]（本轮新增的 ChatMessage 片段）      │
  │  逐条 state.messages.push(msg)           ← 追加到 ContextState         │
  │  再 serde_json 写入 transcript 中尚未落盘的 ChatMessage 行              │
  └─────────────────────────────────────────────────────────────────────────┘
                          │
                          ▼
  ┌─────────────────────────────────────────────────────────────────────────┐
  │ ⑤ LLM 回复后：上下文管理检查（绝不阻塞 UI）                               │
  │                                                                         │
  │  此时 messages 已包含刚完成的 turn 的所有消息。                           │
  │                                                                         │
  │  → preheat.try_restart_if_pending(...)（⑤ 与 ② 双点恢复）               │
  │  → 若 ratio >= 0.85：Layer 2 回复后检查                                 │
  │    （preheat 已有结果？→ Boundary 切换 → Layer 0；未完成→跳过）            │
  │  → 计算新 ratio → 若 >= 0.50：`preheat.try_start(...)`（异步预热，不等待） │
  └─────────────────────────────────────────────────────────────────────────┘
                          │
                          ▼
               用户阅读回复、思考、输入下一条消息
              （异步预热在此期间后台运行）
                          │
                          ▼
  ┌─────────────────────────────────────────────────────────────────────────┐
  │ ② 下一个 user turn 进入前（用户按下回车）                                 │
  │                                                                         │
  │  ◆ 发起 LLM 请求前检查：                                                │
  │    → preheat.try_restart_if_pending(...)                                │
  │    → 若 ratio >= 0.70：Layer 2 检查（完成则 Boundary 切换 → Layer 0）     │
  │    → 若 ratio >= 0.98：Layer 2 发请求前检查                              │
  │      （完成→切换；未完成→化异步为同步，阻塞等待）                        │
  │                                                                         │
  │  若 boundary 已切换：从已更新的 state.messages 重新拍平                   │
  │       + 注入 system prompt + 已持久化的本轮 user 输入                     │
  │       ──► initial_messages: Vec<ChatMessage>                            │
  │                                                                         │
  │  AgentLoop::run(initial_messages) → 进入 ③                              │
  └─────────────────────────────────────────────────────────────────────────┘
```

`**messages`（ContextState）与 `messages`（reasoning loop 工作集）的关系**：

- `**ContextState.messages`**：管理**已完成的历史消息**。只在 ② 进入前读取（clone）、④ 结束后追加、⑤ 被 L0/L1/L2 修改。
- `**reasoning loop messages`**：实时工作集，包含历史 + 当前 turn 正在产生的新消息。每次进入 ② 时从 `ContextState.messages` clone 构建。
- **估算更新**：`estimateContextChars` 在 ③ 每次 push 时实时累加，`last_api_usage` 在每次 LLM 响应后刷新。
- **上下文管理与工作集 `messages` 无交集**：⑤ 只会在 `ContextState.messages` 上轮询/应用 Layer 2；Layer 0 仅在该 boundary 成功应用后作为结构性后续操作。reasoning loop 内的工作集 `messages` 不受压缩影响。下一轮 ② 时从更新后的 `ContextState.messages` 重建，自然包含 Boundary 切换后的摘要。

即：`**ContextState.messages` 是持久化 transcript 与 LLM 请求之间的内存层**——负责 Compaction 折叠与估算维护；`ChatMessage` 已是 LLM wire 格式，无需额外转换。

#### 5.6.1 过渡契约：合并视图事务（2026-09-14）

上面的“上下文管理与工作集无交集”只适用于压缩发生在 user turn **之间**的早期实现。
current-tail guard 和 timing ⑤ 可以在 turn 内消费异步预热结果，因此不能再假定
`covered_end_id` 一定位于 `ContextState.messages`。

```text
before   working = [system][历史][当前 tail]     state = [历史]
fold     state   =           [历史][当前 tail]    cursor = 当前 tail 起点
apply    state   =           [摘要][幸存 tail]    apply_boundary 同步平移 cursor
L0       只处理 state[..cursor]                  幸存 tail 不被落盘或占位
unfold   state   =           [摘要]               working = [system][摘要][幸存 tail]
```

`apply_ready_preheat(agent, messages, min_ratio)` 是 mid-turn 与 timing ⑤ 唯一允许消费
ready preheat 的入口。它在应用前 fold，在应用和 Layer 0 后按平移后的游标 unfold；
因此 `covered_end_id` 查找、摘要替换、L0 的历史边界共用同一个坐标系。`history_end`
是 L0 的硬上界，不再依赖 `keep_recent_turns` 恰好覆盖当前 tail。

timing ⑤ 的 `try_restart_if_pending` / `try_start` 必须快照 working messages（排除 system
prompt）：Scheme E 只允许把 marker 追加到 transcript 尾，而刚完成的 assistant 消息正是
该尾部。这个预热可以覆盖刚结束的 turn；若产品需要“最近一回合一定 raw”，须单独放宽
Scheme E、允许在指定 message 后插入 marker，不能悄悄改回旧快照。

#### 5.6.2 列表归属决策记录

**问题和证据。** 以前，`start_midturn_preheat_if_needed` 从 working messages 取得
`covered_end_id`，而 `apply_and_emit_boundary` 在落后一整个 tail 的
`ContextState.messages` 中定位它。这会确定性触发 `ApplyBoundaryStale`，而不是可由用户
修复的输入错误。

**调研。** Codex 在
`codex/codex-rs/core/src/context_manager/history.rs::ContextManager::record_items` 中即时更新
权威 history，再由 `for_prompt` 派生请求视图；Claude Code 的
`cc-fork-01/src/services/compact/sessionMemoryCompact.ts` 使用
`lastSummarizedMessageId`，锚点失配时回退同步压缩；Pi 在
`pi/packages/coding-agent/src/core/compaction/compaction.ts` 按 entry id 压缩，失配时丢弃
过期候选；OpenCode 从 DB 读取会话历史后在
`opencode/packages/opencode/src/session/compaction.ts` 同步压缩。没有参考实现把“权威列表
落后于请求视图”当作正常状态后再向用户报错。

**当前决策。** 先用本节的合并视图事务止血；随后迁移到单列表：回合间列表停在
`ContextState.messages`，回合内 move 给 `AgentLoop` 独占。届时保留游标和
`history_end` 不变量，删除 fold / unfold / rebuild 胶水。

**反例与推翻条件。** 当前 serve 在回合中只读 metrics，不读完整消息列表，所以不需要
`Arc<Mutex<Vec<ChatMessage>>>`。若未来增加回合中的外部全文读取或写入（例如实时 `/context`
查看或非-steering 注入），应改用带明确并发协议的共享权威状态，而不是恢复双列表。

### 5.7 消息级 ID 与 Compaction 一致性

本节约定 **MessageId**（`ChatMessage.msg_id`）与 **Compaction** 之间的 id 体系，用于解决摘要应用失败、restore 失败、水位下降不及预期等与 **id 不一致或 transcript 行序** 相关的问题。**实现须与本文对齐**（实现排期独立于本文档迭代）。

**总览（ASCII）**——从左到右：内存 `messages` → 预热快照 `S`/`E` → 切口标记 → Layer 2 正文到达与替换。

```
  messages: Vec<ChatMessage>（Layer 1 克隆的 snapshot 示意）
  ═══════════════════════════════════════════════════════════════════

   msg[0]（首条 turn start）     …  中间若干条 …           msg[n-1]（末条）
   msg_id = S                                              msg_id = E
        │                                                    │
        └──────────── snapshot 边界：首条的 S、末条的 E ────┘
                                    │
                                    ▼
                    ┌───────────────────────────────┐
                    │  covered_start_id = S         │
                    │  covered_end_id   = E         │
                    │  BranchSummaryEntry.id = S::E    │
                    └───────────────┬───────────────┘
                                    │
        transcript JSONL（append-only 行序示意）
        ═══════════════════════════════════════════════════════
        … Msg … Msg(id=E)                       ← 触发预热时 E 必须是尾条
                    │
                    │ 前台 O(1) append marker
                    ▼
              branch_summary { id:S::E, summary:null, isBoundary:true }
                    │
        … 工具 / 用户新消息 …                    ← 活尾继续 append
                    │
                    │ 预热完成且前台决定应用时 O(1) append body
                    ▼
              branch_summary_text { forId:S::E, summary:"..." }

        reload 先建 forId → summary 查表；fold 读到 marker 才把正文放回切口。
        Layer 2：messages 中找 end_idx 使 msg.msg_id == E，
                messages.splice(..=end_idx, [ChatMessage::compaction_summary(text, S::E)])
```

**读图要点**：每条 `ChatMessage` 通过 `msg_id` 标识；**`BranchSummaryEntry.id` = 整段快照首条消息的 `msg_id`（`S`）与末条消息的 `msg_id`（`E`）拼成的 `S::E`**。下文 **5.7.1～5.7.6** 为逐条规范。

#### 5.7.1 标识符定义

- **MessageId**：与 transcript 中 `MessageEntry.id` 一一对应（每条 user / assistant / tool 行一条），对应 `ChatMessage.msg_id`（`#[serde(skip)]`，仅内存持有）。**会话内唯一**；字符集**不得**包含子串 `::`（与复合 TurnId 分隔符冲突）。若未来需任意字符串 id，则以 **`covered_start_id` / `covered_end_id` 双字段** 为准、避免依赖拼接解析。
- **TurnId（复合）**：`turn_id := start_msg_id + "::" + end_msg_id`（固定分隔符 **`::`**）。
- **Turn 边界（内存）**：`messages: Vec<ChatMessage>` 扁平存储，turn 边界通过 `is_turn_start()` 动态判定：`(role == User && kind != Steering) || kind == CompactionSummary`。每条 `ChatMessage` 的 `msg_id` 对应 transcript 行 id。
- **`fold` 路径**：在 **从 JSONL 恢复**（`fold_entries_to_messages`）时须为每条 `ChatMessage` 设置 `msg_id`，使内存与磁盘可逆对齐。

#### 5.7.2 预热快照（Layer 1）

Layer 1 启动时克隆 `messages: Vec<ChatMessage>` 为 snapshot，并记录：

- **`covered_start_id`** = snapshot 中第一条**非** `CompactionSummary` 的 `ChatMessage.msg_id`（记为 **`S`**）。
- **`covered_end_id`** = snapshot 中最后一条**非** `CompactionSummary` 的 `ChatMessage.msg_id`（记为 **`E`**）。

边界均为 **message 级** MessageId（`ChatMessage.msg_id`）。

#### 5.7.3 `BranchSummaryEntry` / `CompactionResult` / 摘要消息

- **`BranchSummaryEntry.id`（必填）**：**`S::E`** —— 即 **`covered_start_id::covered_end_id`**，其中 `S`/`E` 来自 §5.7.2 的快照首尾 `msg_id`。
- **`BranchSummaryEntry.covered_start_id` / `covered_end_id`**：分别等于 **`S`** / **`E`**，与将 `BranchSummaryEntry.id` 按 `::` 拆出的左、右段一致。
- **`CompactionResult`**：上述三字段与切口标记一致；`transcript_compaction_entry_id` 必须存整串 `S::E`，作为 marker id。
- **摘要消息的 `msg_id`**：成功 apply 后，`ChatMessage::compaction_summary(text, marker_id)` 使用 `S::E`。摘要没有这条 durable id 就不得构造或作为普通 message 持久化。

#### 5.7.4 方案 E：标记先占位、正文后到达

本节以 [方案 E 整改计划](/Users/yankeben/.cursor/plans/transcript_regressions_remediation_df3378d3.plan.md) 为准。旧的「后台中段插入候选行、再原地翻转 `isBoundary`」方案已移除；`set_branch_summary_entry_is_boundary_true`、`remove_branch_summary_entry_by_id` 与 `write_boundary_transcript` 不再存在。

```text
触发预热（前台，covered_end 仍是 transcript 尾）:
  append branch_summary { id:S::E, summary:null, isBoundary:true }
  └─ 后台只计算摘要，完成后只覆盖写 preheat cache

应用预热（前台，covered_end 仍在 messages）:
  append branch_summary_text { forId:S::E, summary:"..." }
  messages.splice(..=end_idx, [compaction_summary(text, S::E)])
```

两次 transcript 写入都是 O(1) append：marker 在唯一「切口等于文件尾」的瞬间占据正确物理位置；正文稍后作为 tail event 通过 `forId` 关联回 marker。后台永不写 transcript，避免与前台 append 竞争。

#### 5.7.5 Fold、陈旧结果与崩溃恢复

- **两遍 fold**：先收集所有 `branch_summary_text` 为 `forId -> summary`，再按物理顺序折叠。`isBoundary=true` marker 只有 inline summary 或关联正文存在时才清掉前缀并放入摘要；悬空 marker 是 no-op。正文行本身永不进入 timeline、也绝不能触发 clear。
- **陈旧结果**：`covered_end_id` 不再位于当前 `messages` 时，不写正文、不改内存，发 `CompactionError`。marker 留在 transcript 中但因无正文而安全 no-op。
- **崩溃窗口**：后台算完、前台应用前崩溃时，`<session>.preheat.jsonl` 保存一个原子覆盖的 pending result。加载只在 marker 存在、无正文、`forId` 与 `covered_end` 均匹配时 `restore_completed`；其他缓存一律忽略。
- **兼容旧记录**：旧的 inline `branch_summary` 保持原语义；历史 `isBoundary=false` 记录继续仅作为旧 preheat pending 读取。

#### 5.7.6 验收要点

- marker 必须紧接 `covered_end`；后台生成期间 transcript 不变。
- 同一 marker 至多一条 `branch_summary_text`，且绝不产生 `role:user kind:compaction_summary` message 行。
- marker 有正文时重载还原为摘要加活尾；无正文时不得丢失任何原消息。
- 正文已到达时缓存不得再次恢复；`forId` 不符的缓存也不得恢复。

> **与 [session-storage.md](session-storage.md) 的关系**：会话级 `SessionEntry` 中的 compaction 累计字段仍以 user turn 末刷盘为准；MessageId 体系主要约束 **transcript JSONL** 与 **`messages`**，二者交叉引用即可。

---

## 6. 防护算法（Layer 0~3）

### 6.1 Layer 0：与 boundary 成功应用结构性绑定

Layer 0 **不是独立的水位触发器**。它只能紧接 Layer 2 的 `apply_boundary` 成功分支运行：②（回合开始前）、⑤（回合结束）和 mid-turn current-tail guard 三条路径都调用同一个 `apply_and_emit_boundary`，因此三者要么同时完成「摘要应用 → L0」，要么全部不改历史。

这样安排的原因是缓存成本：L0 会改写工具结果，而成功的 boundary 已经替换历史前缀；把两次改写绑定在一起不会额外破坏缓存。相反，boundary 未成功（低水位、无预热结果或 stale）时必须保持历史字节级不变。

```
apply_boundary 成功
      │  （同时 invalidate_api_usage，随后水位回退到字符估算）
      ▼
run_layer0_cleanup
      ├─ A. 上一个已完成 turn 的超大 tool_result 落盘 + preview
      ├─ B. compactable zone 的大 tool_result 替换为 placeholder
      ├─ 作废被驱逐 tool_call 的 ReadFileState stamp
      └─ 累加释放量并在非零时发 layer0_context_release
```

回合执行期间 `ContextState.messages` 只含已完成的历史；当前回合的新消息还在 agent loop 的局部 `messages`，直到 `run()` 收束才同步回来。因此 A 所称的「最后一个 turn」是 **state 中最后一个已完成 turn**，天然可能比当前 in-flight turn 滞后一回合。

**步骤 A：大结果落盘**

在 `messages` 中找到最后一个 turn start（通过 `is_turn_start()` 判定），扫描 `messages[last_turn_start..]` 中 `role == Tool` 的消息：单条 tool_result >= `layer0_single_result_max_chars`（默认 **50K chars**，~12.5K token）→ 落盘 + 500 chars preview 占位符（通过 `ChatMessage::set_text_content(preview)` 原地替换）。**先让 LLM 看到完整内容，再收纳落盘**——LLM 在本轮已正常分析和使用了完整结果，落盘是为了未来轮次的上下文不膨胀。

```
fn persist_tool_result(result: &ToolResult, agent_trail_dir: &Path, session_id: &str) -> String:
    let path = agent_trail_dir
        .join("tool-results")
        .join(session_id)
        .join(format!("{}.txt", result.tool_call_id))
    fs::write(&path, &result.content)

    let preview = &result.content[..min(500, result.content.len())]
    format!("[Tool result persisted: {} ({} chars)]\\nPreview: {}...",
            path, result.content.len(), preview)
```

`agent_trail_dir` 当前默认解析为 `~/.tomcat/agents/{agent_id}/`。旧的 `agent_definition_dir/workspace/{session_id}/tool-results` 仅作为历史迁移来源，不再作为新写入目标。

**步骤 B：compactable zone 占位符替换**

compactable zone（`messages[..protected_start]`，其中 `protected_start = find_protected_turn_start()`）中 `role == Tool` 且 tool_result >= `layer0_placeholder_threshold_chars`（默认 **10K chars**）→ 占位符替换（不落盘），通过 `ChatMessage::set_text_content(PLACEHOLDER)` 原地替换。

```
const PLACEHOLDER: &str = "[Previous tool result replaced to save context space]";

fn compact_old_tool_results(state: &mut ContextState) -> usize:
    let protected_start = find_protected_turn_start(&state.messages, 5)
    let mut reduced = 0

    for msg in state.messages[..protected_start]:
        if msg.role == Tool:
            if msg.text_content().len() >= 10_000:
                if msg.text_content().starts_with("[Tool result persisted:") || msg.text_content() == PLACEHOLDER:
                    continue  # 已处理
                let before = msg.text_content().len()
                msg.set_text_content(PLACEHOLDER)
                reduced += before - PLACEHOLDER.len()
                state.estimate_context_chars =
                    state.estimate_context_chars.saturating_sub(before - PLACEHOLDER.len())
    return reduced
```

**步骤 C：写入 transcript JSONL**

Layer 0 处理后的新 message entry 写入 transcript（落盘的 tool_result 以 preview 占位符形式写入，确保 transcript 中记录的是处理后的版本）。

**步骤 D：重新估算 tokens 和水位**

更新 `estimate_context_chars`，重新计算 `usage_ratio()`，供后续 Layer 1/2 触发判断。

**留 preview 的理由**：仅靠路径和工具名，LLM 在未来轮次无法判断内容是否与当前任务相关。500 chars 的 preview 成本极低（~125 token），但能帮助 LLM 决定是否需要按需读回。

### 6.2 Layer 1：异步预热

时机⑤先 **`preheat.try_restart_if_pending(...)`**，再非阻塞检查 Layer 2；无论 boundary 是否切成，最后按更新后的 ratio 判断 `preheat.try_start(...)`。若 Layer 2 切成，期间已同步完成 Layer 0，再启动下一轮预热；若未切成，L0 不会单独运行。

**主线程不等待**，当前 user turn 处理完毕。异步预热在用户阅读/思考/输入期间后台运行。

```
fn layer1_preheat(state: &mut ContextState, compaction_provider: Arc<dyn LlmProvider>, config: &ContextConfig, ...):
    state.preheat.try_restart_if_pending(state, compaction_provider, config, ...)  # 与 ② 对称；见 §6.6

    if state.usage_ratio() < 0.50:
        return
    if state.messages.is_empty():
        return

    # try_start 内部：Idle 且无双任务时克隆 messages snapshot、spawn task；
    # task 内 generate_summary 最多重试 3 次（§6.6）；成功且 append 成功（或无 transcript 路径）时发 AutoCompactionEnd；耗尽发 CompactionError 并转入 ExhaustedPending
    state.preheat.try_start(state, compaction_provider, config, ...)
```

**异步任务单例性**：

- 由 `Preheat` 内部状态保证：同一时间至多一个 `Running` task
- 防止 51%、52% 连续触发多个 Task 导致 token 浪费和竞态
- 任务结果被 Layer 2 `poll_result` / `await_result` 消费并 `apply` 后，`preheat` 回到可再次 `try_start` 的状态；**`ExhaustedPending`** 依赖 **`try_restart_if_pending`**（⑤ 与 ②）恢复

### 6.3 Layer 2：检查与应用

Layer 2 在 **两个时机** 检查预热结果是否可取用，对应步骤⑤和②；两时机均应先 **`preheat.try_restart_if_pending`**（与 §6.6 一致）。

#### LLM 回复后检查（ratio >= 0.85）

在时机⑤，Layer 2 使用 **`poll_result()`**（或等价非阻塞路径），`Completed(result)` 则应用，否则跳过，绝不阻塞主线程。成功应用时，`apply_and_emit_boundary` 在内部立即执行 Layer 0；失败时 Layer 0 绝不运行。

```
fn check_preheat_after_reply(state: &mut ContextState):
    state.preheat.try_restart_if_pending(...)
    if state.usage_ratio() < 0.85:
        return
    if let PreheatOutcome::Completed(_) = state.preheat.poll_result() {
        apply_boundary_switch(state)
    }
```

> 不区分 ratio 是否 >= 0.98——高水位时尽早非阻塞应用可减少下一轮 ② 发请求前同步等待的概率。

#### 发起 LLM 请求前检查（ratio >= 0.70）

```
fn check_preheat_before_request(state: &mut ContextState):
    state.preheat.try_restart_if_pending(...)
    let ratio = state.usage_ratio()
    if ratio < 0.70:
        return

    match state.preheat.poll_result() {
        PreheatOutcome::Completed(_) => apply_boundary_switch(state),
        PreheatOutcome::NotReady => {
            if ratio >= 0.98 {
                # 化异步为同步：await_result / 阻塞直至完成或失败
                if let PreheatOutcome::Completed(_) = state.preheat.await_result() {
                    apply_boundary_switch(state)
                }
            }
        }
        _ => {}
    }
```

#### Boundary 切换动作（三个检查时机共用）

```
fn apply_and_emit_boundary(state, result):
    # 在 messages 中找 end_idx 使 msg.msg_id == covered_end_id（见 §5.7.5）；无匹配 → ApplyBoundaryStale（§5.7.5.1）
    let end_idx = state.messages.iter().rposition(|m| m.msg_id == result.covered_end_id)
        .ok_or(AppError::ApplyBoundaryStale)?

    # marker 已在触发预热时追加；这里仅追加链接正文。同一 marker 已有尾条正文则不重写。
    append branch_summary_text { forId: result.transcript_compaction_entry_id, summary: result.summary_text }

    let batch_chars = sum(state.messages[..=end_idx].map(|m| estimate_msg_chars(m)))
    let summary_chars = result.summary_text.len()
    let summary_msg = ChatMessage::compaction_summary(&result.summary_text, marker_id)

    state.messages.splice(..=end_idx, [summary_msg])
    # 注意：使用 saturating_sub 防止 usize 下溢（累积估算误差可能导致 batch_chars > estimate）
    state.estimate_context_chars = state.estimate_context_chars.saturating_sub(batch_chars)
    state.estimate_context_chars += summary_chars

    invalidate_api_usage(state)
    run_layer0_after_boundary(state)
```

### 6.4 Layer 3：物理截断（防御性兜底）

API 返回 Context Overflow 错误时触发。从 `messages` 头部起，逐 turn drain（找到第一个 turn 的结束位置），**直到 ratio < 0.50**。

```
fn force_drop_oldest_to_target(state: &mut ContextState):
    while state.usage_ratio() >= 0.50 && !state.messages.is_empty():
        # 找到最老 turn 的结束索引：从 messages[1..] 找下一个 turn start，drain 该范围
        let turn_end = find_next_turn_start(&state.messages, 1).unwrap_or(state.messages.len())
        let removed: Vec<_> = state.messages.drain(..turn_end).collect()
        let removed_chars = sum(removed.map(|m| estimate_msg_chars(&m)))
        state.estimate_context_chars =
            state.estimate_context_chars.saturating_sub(removed_chars)
    invalidate_api_usage(state)
```

**为什么目标是 0.50 而不是刚好 < 1.0**：若只降到 < 1.0，下一条消息或工具调用就可能再次触发 Layer 3，形成频繁振荡。删到 0.50 一次性创造充足缓冲，远低于 Layer 1 首次触发线（0.50），确保 Layer 3 触发后有足够的对话增长空间。

**设计定位**：几乎不可达的安全网。正常运行中，0.50 的 Layer 1 异步预热 + Layer 2 应用通常已足够将 ratio 降回 0.1~0.2。Layer 3 是最后兜底。

#### 6.4.1 Context Overflow 自动重试时的 `messages` 重拼（图二，与 `run.rs` 一致）

在 reasoning loop 的 **可重试**路径中，当错误被判定为 **Context Overflow** 且已执行 **`force_drop_oldest_to_target`**（内部先 **`invalidate_api_usage`**，见 [`cascade.rs`](../../../src/core/compaction/cascade.rs)）后，**同一次 LLM 调用**重试前将工作集 **`messages`** 重组为：

1. **可选保留首条 `System`**：若 `messages[0]` 为 system 消息，则拷贝到重建列表首部（保留系统提示不被折叠丢失）。
2. **`build_context_from_state(ctx_state)`**：直接返回 `state.messages.clone()`（已经是 drain 后的历史上下文）。
3. **尾部原位保留**：`messages[self.context_tail_start ..]`（当前 user turn 在 overflow 前已产生的 assistant/tool 等），与 `start_idx` 更新配合，避免截断正在进行的轮次。

上述顺序在实现中为：`rebuilt = [optional System] + build_context_from_state + tail`；随后写回 **`messages`** 并调整 **`start_idx`**。这与 **§5.6** 中「`**ContextState.messages**` 在 ⑤ 被 L3 修改、下一轮 ② 从之重建」的模型一致，但 overflow 重试发生在 **同轮**内，故需显式重拼 **`messages`**。

> **Layer 3 不受 m 值保护区约束**：当所有 turn 都在 protected zone 内（`protected_start = 0`）时，Layer 0/1/2 无法工作。Layer 3 作为最后兜底，**必须能删除任何 turn**（包括 protected zone 内的），否则极端场景下无法降压。

### 6.4.2 手动 `/compact` 与自动 overflow 恢复不是同一条路径

两者都在“让下一次请求的上下文变小”，但触发时机和优先目标不同，故意不强行复用：

```
用户主动整理会话                         当前请求已经被上游拒绝
        │                                           │
        ▼                                           ▼
/compact                                      Context Overflow
        │                                           │
        ▼                                           ▼
生成可持久化摘要 + 写 branch_summary boundary    先 force_drop_oldest_to_target
        │                                           │
        ▼                                           ▼
重载会话：以后每轮都从摘要继续                 同一轮立即重组更小 payload 并重试
```

| 场景 | 目标 | 实现 | 持久化语义 |
|---|---|---|---|
| 手动 `/compact`（CLI / serve） | 把会话的**长期基线**变小，同时尽可能保留已经完成工作的可读摘要 | [`cmd_compact::compact_session`](../../../src/api/chat/commands/cmd_compact.rs)：先做可丢弃的大工具结果清理，再 `generate_summary`，最后 `append_compaction_boundary` | 是。`branch_summary` 成为新的 transcript boundary，重进会话仍从摘要恢复。 |
| 自动 overflow 恢复 | 让**本次已经失败的请求**获得一个立刻可发送的 payload | [`force_drop_oldest_to_target`](../../../src/core/compaction/cascade.rs)；第二次 overflow 改走 [`collapse_to_branch_summary`](../../../src/core/agent_loop/current_tail_guard.rs) | 首次 L3 截断只改内存；第二次 Collapse 使用摘要收敛工作集。这里优先保证请求能继续，不承诺保存全部逐条历史。 |

因此“手动命令没有调用 `force_drop_oldest_to_target`”不是实现漏接：前者需要一个能跨重启解释既往工作的 checkpoint，后者需要在错误返回后的最短路径内腾出空间。两条路径共用 `ContextState`、摘要生成能力和 `usage_ratio` 作为结果度量，但不共享触发策略。

验收不以“命令成功”或“boundary 已写入”为准：CLI 重载后的 `usage_ratio`，以及 serve 响应内的 `afterUsageRatio`，都必须严格小于压缩前的值。

### 6.5 防振荡设计

落盘后如果 LLM 再次全量读取同一文件，新 tool_result 仍可能超阈值、再次落盘，形成「读 → 落盘 → 再读 → 再落盘」的无效循环。

防范策略：

1. **分页读取引导**：system prompt 中明确告知 LLM「已落盘的工具结果可通过 `read_file` 的 offset/limit 参数按需读取指定行范围，无需全量读取」
2. **占位符自包含**：preview（前 500 chars）+ 来源工具名 + 参数 + 大小，让 LLM 有足够信息决定是否需要读回、读哪部分
3. **兜底保障**：即使 LLM 仍然全量读取，Layer 0 会再次正常落盘。流程上不会死循环（每轮仍正常推进），只是浪费了一次全量读取的 token。这属于 LLM 行为问题，通过优化 system prompt 引导来改善，不需要在代码层做硬拦截

### 6.6 异步预热失败处理（Preheat 状态机）

Layer 1 由 **`Preheat`** 封装，取代原先在 `ContextState` 上直接持有 `Option<CompactionResult>` 的做法。

**内部状态（实现细节，不对外暴露）**：`Idle` / `Running` / `ExhaustedPending`。

**对外 API（仅此与预热交互）**：`try_start`、`try_restart_if_pending`、`poll_result`、`await_result`、`abort`。

**3× retry（在 spawn 的 task 内部）**：

- 对 `generate_summary` **最多连续尝试 3 次**。
- **成功**：在 **`append_entry` 成功**（或无 transcript 路径）后发出 **`AutoCompactionEnd`**（L1 可观测性），返回 `CompactionResult`，供 Layer 2 Boundary 应用。**不在** Layer 2 `apply_boundary` 路径重复发射 `AutoCompactionEnd`。
- **三次均失败（耗尽）**：发出 **`CompactionError`**，含 `exhausted_after_retries: true`、`attempts: 3`、`source: "preheat"` 等字段；task 以 **`Err`** 结束；状态转入 **`ExhaustedPending`**（不会在同一失败点自动再 spawn，需走恢复路径）。
- **`apply_boundary` 失败**：
  - **`AppError::ApplyBoundaryStale`**（`covered_end_id` 在当前列表不可解析）：发出 **`CompactionError`**（`source: "apply"`、`exhausted_after_retries: false`），**删除** transcript 中对应 **`branch_summary`** 行（见 **§5.7.5.1**），**不** **`restore_pending_result`**；可选 **`discard_cached_completed`** 防御清理。
  - **其它可重试错误**（未来若扩展）：仍 **`restore_pending_result`**，与旧「待消费 `CompactionResult`」语义一致。

**⑤ + ② 双点 `try_restart_if_pending`**：

- 时机 **⑤**（LLM 回复后）与时机 **②**（下一次发起 LLM 请求前）**都调用** `preheat.try_restart_if_pending(...)`。
- 这样即使 **⑤ 未执行到**（例如走了 **tool_calls** 分支、提前结束本轮），仍可在 **②** 补上恢复，避免长期卡在 `ExhaustedPending`。

**`abort`**：

`preheat.abort()` 将状态从 **任意** 状态收束到 **`Idle`**：取消运行中的 task、清除 pending / 未完成句柄，与 Session 销毁或用户中止时释放资源一致。

### 6.7 与 `max_tool_rounds` 的关系

> **TODO**：`max_tool_rounds` 硬限暂时**移除**（不再限制 reasoning loop 工具轮数）。防死循环由上下文预算 + 后续独立的 tool-loop-detection 方案负责。等 tool-loop-detection 方案落地后再评估是否需要恢复硬限。

**对照 openclaw / pi-mono**

- **openclaw**：无对等固定轮数上限；靠 **tool-loop-detection**（重复/无进展/乒乓）+ **tool-result 上下文守卫** + **Compaction / overflow 恢复**组合约束。
- **pi-mono（coding-agent）**：无 `max_tool_rounds`；由 **token 预算 + Compaction** + 用户中止约束行为。

两者均**不**硬编码轮数上限，tomcat 对齐此策略：工具轮次受 **上下文预算（本方案）** 自然约束——token 用尽时 Compaction 压缩或兜底中止，无需额外硬限。

---

## 7. Compaction 摘要模板

### 7.1 首次摘要（SUMMARIZATION_PROMPT）

```
Create a structured context checkpoint summary that another LLM will use to continue the work.

Use this EXACT format:

## Goal
[What is the user trying to accomplish? Can be multiple items.]

## Progress
### Done
- [x] [Completed tasks/changes]

### In Progress
- [ ] [Current work]

### Blocked
- [Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Ordered list of what should happen next]

## Critical Context
- [Any data, examples, or references needed to continue]
- [Or "(none)" if not applicable]

Keep each section concise. The entire summary should be under ~8K tokens.
Preserve exact file paths, function names, and error messages.
Prioritize actionable information over verbose descriptions.
```

### 7.2 Compaction 模型选择


| 策略               | 说明                                                         | 推荐        |
| ---------------- | ---------------------------------------------------------- | --------- |
| **默认：`gpt-5.2`** | 作为默认压缩模型，固定给摘要/keepalive 路径使用；若想与主对话同模，可显式把 `compaction_model` 改成当前 `model` | **当前默认**  |
| 与主对话对齐           | 将 `compaction_model` 设为与 `model` 相同，行为与「全用主模型」一致           | 可选        |
| 轻量模型             | 如 `gpt-4o-mini` / DeepSeek-V3，成本低但需自行评估摘要质量                | 成本敏感时可改配置 |


> 实现上 Compaction 的 LLM 调用使用 `**compaction_model`**，与 `ChatRequest.model`（主对话）分离配置；若希望完全一致，将两项设为同一模型 ID 即可。

### 7.3 增量更新（UPDATE_SUMMARIZATION_PROMPT）

当 `messages` 中已有上一次 `kind == CompactionSummary` 的摘要消息时，采用 UPDATE 模式合并：

```
Update the existing structured summary with new information. RULES:
- PRESERVE all existing information from the previous summary
- ADD new progress, decisions, and context from the new messages
- UPDATE the Progress section: move items from "In Progress" to "Done" when completed
- UPDATE "Next Steps" based on what was accomplished
- PRESERVE exact file paths, function names, and error messages
- If something is no longer relevant, you may remove it
- The complete updated summary (which REPLACES the old one entirely) should be under ~8K tokens
- When the old summary is already large, compress older/less relevant details to stay within budget

Use this EXACT format (same as the original summary):

## Goal
[Updated goal]

## Progress
### Done
- [x] [Completed tasks]

### In Progress
- [ ] [Current work]

### Blocked
- [Issues, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Updated ordered list]

## Critical Context
- [Updated references]
```

### 7.4 摘要消息格式

方案 E 的一个压缩批次由两条 append-only JSONL entry 表示：

- **预热触发阶段**（Layer 1 spawn 前）：前台确认 `covered_end` 是 transcript 尾条后，追加 `type: branch_summary` marker（`id:S::E`、`summary:null`、`isBoundary:true`）。
- **应用阶段**（Layer 2 Boundary 切换时）：前台追加 `type: branch_summary_text`（`forId:S::E`、完整正文），再替换内存中的被压缩区间；后台异步任务不写 transcript。

重载时先收集正文，再在 marker 的物理位置折叠。marker 没有正文时是 no-op；正文行本身不生成消息。在内存中摘要是 `ChatMessage::compaction_summary(text, marker_id)`（`role=user`，`kind=CompactionSummary`），替换被压缩的原始消息。

### 7.5 Compaction v2 修订（T2-P0-002）

> 本小节为 **T2-P0-002**（compaction prompt 与 context v2）的最小落档；§7.1 / §7.3 的中间形态已被本次升级取代，**单一事实来源**为 [`src/core/compaction/preheat.rs`](../../src/core/compaction/preheat.rs) 中两个 `pub(super) const` 字面量。

#### 7.5.1 结构化模板

`SUMMARIZATION_PROMPT` / `UPDATE_SUMMARIZATION_PROMPT` 要求 `Goal`、`Progress`（Done / In Progress / Blocked）、`Errors Encountered`、`Key Decisions`、`Next Steps`、`Critical Context`，并要求 `Next Steps` 给出 **verbatim 引用**（精确 file path / 函数名 / 错误信息）。`Constraints & Preferences` 和 `Recent User Messages` 不由模型转述：计划路径和用户原话由运行时机器区块逐字提供。两个模板首行均为 `Respond with text only. Do not call any tools.`，指令区追加 `First reason internally, then output the final summary.`（隐式诱导内部推理，不开 Two-pass 第二轮 LLM）。

模板章节顺序与必要约束：

```
## Goal
## Progress    （Done [带 file: 锚点] / In Progress / Blocked）
## Errors Encountered
## Key Decisions
## Next Steps    （含 verbatim 引用）
## Critical Context
```

行为契约：
- `<8K tokens` 的输出预算与既有保持一致；§7.2 模型选择不变。
- 测试锁点见 [`src/core/compaction/tests/prompt_snapshot_test.rs`](../../src/core/compaction/tests/prompt_snapshot_test.rs)（章节标题、text-only 首行、internal-reason 指令、无模型转述的约束/用户原话、verbatim 要求、文件锚点提示、`{existing_summary}` 占位符等）。

#### 7.5.2 Compaction 请求显式 `tools: None`

`Compactor::generate_summary` 构造 `ChatRequest` 时**显式**写 `tools: None`，与模板首行的 text-only 声明形成**双保险**——即使后续模型对 prompt 指令不敏感，因请求体不携带 tool schema，模型亦无能力发起工具调用。它经 `LlmProvider::chat_collect` 取得完整摘要：普通 provider 使用非流式 JSON；拒绝 `stream:false` 的 Responses 中转站则在 adapter 内缓冲 SSE/NDJSON，语义仍是一份完整的 `ChatResponse`，不向摘要状态机泄露传输差异。该约束与 §10.1 中 reasoning loop 的工具调用契约**互不污染**：摘要 LLM 调用走独立的 `ChatRequest`，不复用主对话的请求构造。

#### 7.5.3 失败路径：指数退避 + transcript 留痕

`Compactor::try_start` 内的 `for attempt in 1..=MAX_PREHEAT_RETRIES` 重试 loop **Err 分支末尾**追加 `tokio::time::sleep(500ms << (attempt - 1))`，即 `500ms / 1s / 2s` 三档退避；3 次都失败后通过 `insert_entry_after_message_id` 在 transcript 写一条 `BranchSummaryEntry { summary: None, error: Some(...), attempts: Some(3), is_boundary: false, ... }` 的失败锁点，与既有 `AgentEvent::CompactionError` 事件流并行（事件给上层 UI、留痕给会话重启与人工排查）。

`BranchSummaryEntry` 新增两个可选字段（向后兼容旧 transcript）：

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub error: Option<String>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub attempts: Option<u32>,
```

`init_context_state` fold 时遇 `summary == None` 直接跳过（不生成假摘要 `ChatMessage`，仅作日志锚点），不影响重启路径与既有 §7.4 boundary 升级语义。

测试锁点见 [`src/core/compaction/tests/preheat_and_truncation.rs`](../../../src/core/compaction/tests/preheat_and_truncation.rs) 与 [`src/core/compaction/tests/legacy_transcript_compat.rs`](../../../src/core/compaction/tests/legacy_transcript_compat.rs)（虚拟时钟下退避计时；3 次耗尽端到端写入；缺字段默认 None；成功条目不序列化失败字段）。

#### 7.5.4 提示缓存前缀稳定化

提示缓存只复用逐字节相同的请求前缀。因此系统遵循「已发送历史只追加、运行时事实先在内部请求尾部生成，
再由 adapter 归一化到 instruction 通道」：

```text
tools（稳定排序） → system（稳定说明） → 持久化历史 → 本轮输入 → runtime tail
      ↑                   ↑                         ↑              （未缓存后缀）
 Anthropic 断点 1      断点 2              最深断点 D（写锚）
```

- `TokenUsage` / `StreamEvent::Usage` 记录 provider 返回的 `cache_read_tokens` 与
  `cache_write_tokens`；只写入 assistant transcript 和 `tomcat_chat_diag`，不扩展
  `ContextMetricsUpdate` wire schema。
- 内置工具目录在所有 mode 相同，插件工具按 `(plugin_id, name)` 排序；权限与 mode
  仍由 handler/path gate 强制执行，而不是通过删工具定义来实现。
- `ChatRequest` 构造接缝在每个主 Agent 请求末尾临时添加 runtime tail（计划提醒和当前
  workspace 权限状态）。它不进入 `ContextState.messages` 或 transcript，也绝不作为 provider dialogue
  message：Anthropic wire 把它渲染为不单独标记的 system suffix，Chat Completions 合并到 leading system，
  OpenAI Responses 合并到顶层 `instructions`。稳定时 durable history 只追加；状态变更会有意使下一次缓存冷写。
- Anthropic wire 最多放四个 `cache_control: {type: "ephemeral"}` 断点：tools 末项（A）、
  最后一个非 runtime system block（B）、倒数第二条 wire user 消息末尾（C，滚动读锚）和最后一条持久 wire
  消息末尾（D，写锚）。runtime tail 是不单独标记的 system suffix；它变化时会使下一次 Anthropic 缓存失效。
  OpenAI Chat Completions / Responses 使用按 `{session_id}:{family}` 分域的
  `prompt_cache_key`。
- Layer 0 只在 Layer 2 boundary **成功应用后**运行一次：L0-A 落盘最后一 turn 的超大
  tool result，L0-B 替换 compactable zone 的大 tool result。Layer 1 的 preheat 本身只读
  快照，不会改写缓存；未切换 boundary 时保持历史字节不变，真正改变历史的是 Layer 2/L3
  的必要应用。

#### 7.5.4.1 2026-08-05 Responses 缓存异常：移动 tail 截断 input 前缀

`cache_read_tokens` 是服务端实际命中的**量化后的前缀长度**，不是「本轮新增历史字数」。
因此它可以连续数轮不变；只有服务端选择更深的缓存块时才会跳变。遇到曲线平台时，先验证 wire
是否真的改变了稳定前缀，不能仅凭单个数值推断客户端没有缓存。

```text
旧 Responses wire：

请求 N:    durable history → EphemeralTail（临时 user input）
请求 N+1:  durable history → 新 assistant/reasoning/tool → EphemeralTail
                             ↑
                  取代了请求 N 中 tail 的位置，前缀在此断开

结果：只能读取 tail 之前的静态 system / tool 区（实际约 8.7K）。
```

根因不是 `prompt_cache_key`、完整工具目录或历史 opaque reasoning：

- 固定 cache key 只能选择服务端缓存桶，不能让不同位置的 item 拼接成前缀；
- 完整 agent tool catalog 的 40K 对照请求第二次读取 42,496；
- 真实 transcript 依次保留全部、仅最新或删除 `reasoning.encrypted_content`，均仍停在约 8.7K；
- 真正首个漂移由 `TOMCAT_PROMPT_PREFIX_FINGERPRINT=1` 的滚动 input hash 定位：请求 N 的
  EphemeralTail 对应请求 N+1 的 reasoning / assistant / function call / tool result。

修复将 tail 保持为 request-only，但改由顶层 instructions 承载：

```text
instructions = stable system prompt + runtime tail
input(N+1)   = input(N) + assistant / reasoning / tool result
                         ↑ 仅追加，允许 Responses 自动缓存继续扩展
```

这也校正了旧的简化马拉松结论：该测试把约 35K 稳定文本放在 system instructions 内，报告的深度命中
只能证明 system 命中，不能证明 tail 之后的工具历史被写入后续前缀。

真实 Agent-shaped fcodex gpt-5.6-sol / XHigh 实验：

```text
旧 tail 作为 input user:    0 → 8,704 → 8,704 → 8,704 → 8,704 → 8,704
正式 wire，稳定 tail:        0 → 9,728 → 10,752 → 10,752 → 10,752 → 10,752
正式 wire，每轮变 grant:     0 → 9,728 → 9,728 → 9,728 → 9,728 → 9,728
                                           ↑
                          state 稳定时量化块推进；频繁变 state 不承诺更深命中
```

tail 真正变化（plan mode、workspace permission / session grant 改变）会改变 instructions，因此 fcodex
不承诺该轮的深度命中；状态稳定后的请求仍保持 append-only input，具备恢复增长的必要条件。不得为了
命中率把 workspace state 伪装成普通 user 消息或持久写回 transcript。

最终用用户提供的同一 session JSONL 截到最后一个 tool output，再按真实 Agent loop 继续六个小工具
回合，裁决了「fcodex 是否无法缓存 40K 历史」：

```text
旧 tail-in-input 重放：    0 → 8,704 → 8,704 → 8,704 → 8,704 → 8,704
正式 Responses wire 重放：0 → 48,640 → 48,640 → 48,640 → 48,640 → 48,640
第六轮 prompt_tokens：49,634
```

所以 8.7K 不是 transcript 的固有限制，也不是完整工具目录或 39–44 个历史 reasoning item 的限制；
它是移动 tail 之前的共享前缀。48.6K 后的平台符合量化缓存块行为，不能要求每个小 tool output 都使
`cache_read_tokens` 立即上升。

两个候选方案也已被真机排除：

```text
previous_response_id（HTTP）
  └─ fcodex 返回 400：only supported on Responses WebSocket v2
     └─ 只能回退为显式 replay；默认打开会平白多一次失败请求

explicit breakpoint（GPT-5.6+）
  └─ 合法锚点是 input_text / image / file
     ├─ 最后的持久项常是 function_call_output，不能可靠地写入 cache
     └─ 追加伪造 user checkpoint 虽可改变锚点，但会改变模型语义
```

不向 fcodex HTTP 默认发送 `previous_response_id`，也不注入语义性 checkpoint。
`previous_response_id` 的 opt-in 路径仍只向**确实支持它的路由**发送锚点之后的增量 input，避免把
已经由服务端恢复的历史再次重复发送。

重新评估此决策的条件是：fcodex 宣布 HTTP Responses 支持 `previous_response_id`、OpenAI / 网关确认
`function_call_output` 上的 breakpoint 能产生 cache write，或者生产 fingerprint 显示 tail 已不在 input
但仍固定只命中旧断点。最后一种情况必须继续追踪更早的历史重写项。

用 `scripts/analyze_prompt_cache_baseline.py` 对同一条至少 20 轮真实会话采集改造前后
命中率、授权次数、插件/技能变动、拒绝工具比例与 `usage_ratio` P50/P90。

#### 7.5.4.2 2026-08-03 三条 wire 实测基线

三组探针均通过真实 provider 连发 20 次 `LlmProvider::chat` 请求，并把上一轮 assistant
回复追加到下一轮的消息历史。数据由
`scripts/analyze_prompt_cache_baseline.py` 从 `tomcat_chat_diag` 形状的 usage 日志统计：

| wire / 模型 | prompt tokens | cache read tokens | cache write tokens | 命中率 |
|---|---:|---:|---:|---:|
| DeepSeek Chat / `deepseek-v4-pro` | 118,500 | 111,232 | 0 | 93.87% |
| fcodex OpenAI Responses / `gpt-5.6-sol` | 116,160 | 82,688 | 0 | 71.18% |
| fcodex Anthropic Messages / `claude-opus-4-8` | 208,242 | 208,202 | 11,222 | 99.98% |

Anthropic 的 `input_tokens` 不包含独立返回的 `cache_read_input_tokens`；分析脚本会先把两者相加
为总输入再计算命中率，避免把 20 次 `input_tokens=2` 误读成万倍命中率。

**结论**：三条 wire 都发生了真实 cache read，且 Anthropic 显式断点能够随追加历史推进。
Responses 的 71.18% 明显低于 DeepSeek 和 Anthropic，说明 fcodex Responses 当前只复用有限大小的
前缀块；这不是本地 `prompt_cache_key` 缺失（同一 key 已连续下发），不能把它当成稳定达到 80%
的跨 provider 承诺。后续应在网关侧确认 cache block 上限和前缀扩展策略，再决定是否把该阈值纳入
Responses 的发布门禁。

这些是 provider 级缓存探针，故没有真实工具执行、授权、插件变更或 `ContextMetricsUpdate` 样本；
它们用于验证 wire 和 usage 解析，不替代带工具负载的 Agent E2E 指标。

#### 7.5.5 三项不实施决议（关闭/转后续）

T2-P0-002 立项决议中**明确不做**的三项相关动作（决议另见 [`docs/TODOS.md`](../TODOS.md) 与工程变更记录，背景论证见 [报告 §5.7](../reports/compaction-prompt-cc-vs-pi.md)）：

| 决议 | 范围 | 承接路径 |
| :--- | :--- | :--- |
| `#T-040` 不在 `messages_to_text` 做内容硬截断 | compaction 不兼任输入校验；现有 Layer 0（`>= 50K` 落盘 + 200 字 preview）已覆盖 Tool 路径 | User/Assistant 巨量消息让 LLM 自行返回 `context_length_exceeded`，由 §7.5.3 退避 + 失败留痕路径承接 |
| `#T-043` 不新增 `tool-results/_index.jsonl` 落盘索引 | 信息可由 transcript 占位符 + fs mtime + 文件名（tool_call_id）完全重建，主路径无消费者 | 真实归属 `executor/primitives.rs::edit_file`（agent 写大文件方式），抽出 **T2-P0-011 large-file-edit-strategy** 承接 |
| `#T-044` 不实施 Two-pass summary | CC 用 fork 子代理 + prompt cache 抵消草稿成本，tomcat 单次 LLM 直发性价比不好 | 模板指令区追加 `First reason internally, then output the final summary.` 隐式诱导，不开第二轮 LLM |

---

## 8. 超出本方案范围（Out of Scope）

以下机制在研究报告中分析过，但不纳入本方案实现：


| 机制                              | 说明                                                                                                | 后续计划                     |
| ------------------------------- | ------------------------------------------------------------------------------------------------- | ------------------------ |
| **Snip（中间段删除）**                 | CC Level 1，删除中间历史保留头尾，零 API 成本。当前 Layer 1 异步摘要可覆盖此场景。                                             | 若 Layer 1 触发过于频繁/费用高，再评估 |
| **Prompt Cache 管理**             | Anthropic 使用 `cache_control` 断点，OpenAI 使用自动前缀缓存 + scoped `prompt_cache_key`；见 §7.5.4 | 已实施，按基线数据复测 |
| **Cached Microcompact**         | 依赖 `cache_edits` 服务端打洞能力                                                                          | 不适用                      |
| **Session Stability Latching**  | 锁定运行时状态防 cache bust，tomcat 无 Prompt Cache                                                             | 不适用                      |
| **Context Collapse**            | CC 实验性 commit-log 视图投影，通用性差                                                                       | 不纳入                      |
| **工具循环检测（tool-loop-detection）** | openclaw 的滑窗重复检测 + steering 注入 + 熔断                                                               | 独立方案在 agent-loop 中实现     |
| **RAG 检索增强**                    | 旧消息向量化 + 按相关性检索注入                                                                                 | 长期方向                     |
| **System Prompt 自动注入**          | 从对话中自动提取约束/偏好到 system prompt                                                                      | 可与 Compaction 互补，后续独立方案  |


---

## 9. 涉及改动文件


| 文件                                                                                | 改动内容                                                                                                                                                                        |
| --------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `[src/core/session/manager/types.rs](../../../src/core/session/manager/types.rs)` | `ContextState` 持有 `messages: Vec<ChatMessage>`、`preheat: Preheat`（Layer 1 状态机）、`start_idx` 等；`CompactionResult` 含 `transcript_compaction_entry_id`；`apply_boundary` 仅按 **`covered_end_id`** 定位 `end_idx`（通过 `msg_id` 匹配），无匹配返回 **`AppError::ApplyBoundaryStale`**                                           |
| `[src/core/session/transcript.rs](../../../src/core/session/transcript.rs)` | `BranchSummaryEntry` 是触发预热时追加的切口 marker；`BranchSummaryTextEntry` 是应用时追加并以 `forId` 关联 marker 的正文。已移除原地升级 `isBoundary` 与删除陈旧 marker 的 rewrite API。 |
| `[src/core/agent_loop/turn_finalize.rs](../../../src/core/agent_loop/turn_finalize.rs)` / `[src/core/compaction/apply.rs](../../../src/core/compaction/apply.rs)` | reasoning loop 最终回复后：① 恢复 pending preheat ② ratio check 后 Layer 2 非阻塞检查（成功 boundary 在同一成功分支运行 Layer 0）③ 启动下一轮 Layer 1 预热；发起下一次 LLM 请求前：r>=0.70 时 Layer 2 检查、r>=0.98 时可能同步等待，成功后由调用方重建发送消息 |
| `[src/infra/config/types.rs](../../../src/infra/config/types.rs)`                 | `[context]` 配置节更新 `layer0_single_result_max_chars` 为 50K、新增 `layer0_placeholder_threshold_chars`（10K）、新增 `compaction_max_tokens`（10K）                                       |
| `[src/core/compaction/](../../../src/core/compaction/)`                           | `layer0.rs` 同步清理；`preheat.rs` 前台先追加 marker、后台只计算并写 pending cache；`apply.rs` 三个检查时机共用 `apply_and_emit_boundary`，追加正文后执行 `messages.splice(..=end_idx, [summary_msg])`；`truncation.rs` 物理截断。 |
| `[src/core/system_prompt.rs](../../../src/core/system_prompt.rs)`                 | 新增分页读取引导 section                                                                                                                                                            |
| `[src/infra/events/mod.rs](../../../src/infra/events/mod.rs)`                     | 压缩可观测性：L0 `Layer0ContextRelease`、L1 `AutoCompactionStart`/`End`（含估算三字段）、`CompactionError`、L2 `BoundarySwitched`（含 `estimatedTokensFreed`）、L3 `ContextOverflowTrimStart`/`End`（含删轮与释放）、`context_metrics_update`（详见 §10.4 / §10.6） |
| `[src/core/context_metrics.rs](../../../src/core/context_metrics.rs)`             | `ContextLiveMetrics`（别名 `ContextMetrics`）字段语义；运行时嵌入 `ContextState::live`。**会话累计**在 `ContextState::session_obs`，见 §10.6 |


---

## 10. 与其他模块的关联

### 10.1 Agent Loop（agent-loop.md §13.3）

Agent Loop 中有 **三个检查时机** 与上下文管理交互（对应 §5.6 步骤编号）：

- **⑤ LLM 回复后**（user turn 完成，绝不阻塞）：
  - `preheat.try_restart_if_pending(...)`（与 ② 双点恢复 ExhaustedPending）
  - ratio >= 0.85 → Layer 2 回复后检查（`poll_result` 已有 `CompactionResult` 则立即切换；成功切换在内部紧接执行 Layer 0，非阻塞）
  - ratio check → `preheat.try_start(...)`（Layer 1 异步预热，不等待）
- **② 发起下一次 LLM 请求前**（下一个 user turn 进入时）：
  - `preheat.try_restart_if_pending(...)`
  - ratio >= 0.70 → Layer 2 检查（完成则 Boundary 切换）
  - ratio >= 0.98 → Layer 2 发请求前检查（未完成则**化异步为同步**阻塞等待）
  - 成功 Boundary 切换后，L0 已同步完成；`ContextState.messages` 是历史的唯一事实源头，工作集必须从它重新拍平，并把已持久化的当前用户输入接回尾部
- **③ reasoning loop 内 API 返回 Context Overflow 错误**：
  - Layer 3 物理截断 + 重试

**容错重试循环（第二层）**：LLM 返回 ContextOverflow 错误时，发布 **`context_overflow_trim_start` / `context_overflow_trim_end`**（L3），驱动 Layer 3 物理截断与可选重试；异步预热进度仍由 L1 的 **`auto_compaction_*`** 表示。

### 10.2 会话存储（session-storage.md）

- 压缩摘要以两条 append-only entry 写入 transcript：
  - 预热触发时前台追加 `branch_summary` marker（`summary:null`、`isBoundary:true`、`id:S::E`）。
  - 应用时前台追加 `branch_summary_text`（`forId:S::E`、完整摘要正文）；两遍 fold 在 marker 的位置还原摘要。
- Tool result 落盘文件存储在 `agent_trail_dir/tool-results/{session_id}/` 目录，即默认 `~/.tomcat/agents/{agent_id}/tool-results/{session_id}/`。
- 初始化时从 transcript 流式读取消息（遵守「禁止全量加载」约定，使用 `BufReader` 逐行解析 + `fold_entries_to_messages` 输出 `Vec<ChatMessage>`）；无正文 marker 是安全 no-op，`branch_summary_text` 本身不产生消息。

### 10.3 配置管理（infrastructure-layer.md）

- `[context]` 配置节由 `PrimitiveConfig` 加载，支持 `tomcat.config.toml` 覆盖。
- 新增/更新 `layer0_single_result_max_chars`（50K）、`layer0_placeholder_threshold_chars`（10K）、`compaction_max_tokens`（10K）配置项。
- 不同模型可通过 `[model.<name>]` 节覆盖 `context_window` 和 `max_output_tokens`。

### 10.4 事件系统（events.md）

本模块发布的压缩相关事件按 **L0 / L1 / L2 / L3** 分层（Rust variant ↔ wire name）；字段表与 camelCase 细节以 [events.md](plugin-system/events.md) 为准。


| Layer | Rust variants | wire names |
| ----- | ------------- | ---------- |
| **L0（boundary 应用后同步清理）** | `Layer0ContextRelease { persist_tokens_freed, placeholder_tokens_freed }` | `layer0_context_release` |
| **L1（异步预热）** | `AutoCompactionStart { covered_count, ratio_before }` / `AutoCompactionEnd { elapsed_ms, summary_chars, covered_count, ratio_after, estimated_covered_tokens_before, estimated_summary_tokens, estimated_tokens_saved }` / `CompactionError { exhausted_after_retries, attempts, error, source, ratio }` | `auto_compaction_start` / `auto_compaction_end` / `compaction_error` |
| **L2（边界切换）** | `BoundarySwitched { ratio_before, ratio_after, covered_count, was_sync_wait, estimated_tokens_freed }` | `boundary_switched` |
| **L3（溢出裁剪）** | `ContextOverflowTrimStart { reason, ratio }` / `ContextOverflowTrimEnd { ratio_before, ratio_after, will_retry, estimated_tokens_freed, turns_removed }` | `context_overflow_trim_start` / `context_overflow_trim_end` |


其他与本模块相关的通用事件（未归入上表分层）：`tool_result_persisted`（Layer 0 单条落盘）、`context_metrics_update`（首次 `chat_stream` 前发估算值；每次 provider `Usage` 到达后立即发实测值；timing ⑤、压缩或重载后再发最佳估算快照；payload 中累计字段来自 `ContextState`）等，见 `events` 模块定义。

**CLI / 宿主按 wire name 订阅时的语义对应**：

- `layer0_context_release` → **L0** 本轮落盘 + 占位符释放的估算 tok（已写入会话累计）
- `auto_compaction_start` / `auto_compaction_end` → **L1** 异步预热进度与前/后/差展示（**不在** `auto_compaction_end` 时累加会话 `compactionTokensFreed`）；`auto_compaction_end` 在 L1 计算完成并写入 pending cache 后发射，L1 不写 transcript
- `compaction_error`：`source: "preheat"` 且 **`exhausted_after_retries == true`** → **待恢复**提示（需结合 ⑤/② 的 `try_restart_if_pending` 或用户操作）；`source: "apply"` → apply 失败：若错误为 **`ApplyBoundaryStale`**（列表不可解析），**不**再挂起同一 `CompactionResult` 重试；其它 apply 失败仍表示摘要 **待 `restore_pending_result` 重试**
- `context_overflow_trim_start` / `context_overflow_trim_end` → **L3** Context Overflow 后的物理裁剪与释放量
- `boundary_switched` → **L2** 摘要已应用、边界重置（`estimatedTokensFreed` 与本次计入累计的量一致）


### 10.5 UI 反馈


| ratio 档位            | UI 表现               |
| ------------------- | ------------------- |
| ratio >= 0.50 且预热中  | 状态栏转圈图标："后台准备压缩..." |
| Boundary 切换完成       | 闪过提示："上下文已重置"       |
| ratio >= 0.98 同步等待中 | 状态栏："等待压缩完成..."     |


### 10.6 可观测性与 token 估算约定

- **`context_metrics_update` 节奏**：本轮首次流式请求前先发一条 fallback 估算；随后**每一次** provider `Usage` 事件到达时，stream handler 立即以该实测值发一条更新——它发生在工具调用被调度之前，不能等待整次 Agent 回合结束；timing ⑤、current-tail reduction、L3 截断与 rehydrate 后仍会发出边界处理后的最佳估算快照。故工具轮数本身不决定事件数，provider 实际返回的 usage 次数才决定。`providerUsageMeasured=true` 只表示**本条事件**直接来自刚收到的 provider usage；`false` 表示本条是估算，而不是对已存在实测基线的否定。`compactionCount` / `compactionTokensFreed` / `totalToolResultBytesPersisted`（字段名历史兼容，**值为 Unicode 字符累计**）来自 **`ContextState::session_obs`**；瞬时字段来自 **`ContextState::live`**（含 `preheatInProgress` = 预热 LLM 任务仍在跑；`preheatResultPending` = 摘要已就绪待 poll/apply，与前者互斥；`turnCount` = `state.turn_count()` 统计 `messages` 中的 turn start 数）。`AgentLoop` **不**再持有独立 `metrics` 结构。
- **Ctx% 显示语义**：输入框的 Ctx% 是「本进程当前 slot 最近一次可信占用快照」，不是 `sessions.json` 的历史观测快照。冷启动 slot 的 `last_context_ratio=None` 收到 `providerUsageMeasured=false` 时必须继续留空，避免把 `chars/4` 粗估伪装成首个实测值；provider 返回 usage 后的 `true` 事件定基并显示 `Ctx x%`。一旦已定基，后续 `false` 估算（下一条 prompt、系统提示刷新、L0/L2/L3、rehydrate 或模型预算变化）必须更新该快照而**不得隐藏**，直到下一次 provider usage 再以真值刷新。
- **重启、重连与改写历史**：新 slot 的 `last_context_ratio=None`，`get_state` 返回 `null`，Composer 隐藏 Ctx 文字但以固定宽度保留位置，避免按钮跳动。相同进程中切换或重连已测过的会话可从 slot 缓存立即回放；`/compact`、`/restore` 与其他 `rehydrate_slot_context_state` 路径会用重建后的上下文主动发估算事件：已定基的会话立即显示新水位，未定基会话仍留空。`SetModel` 同理：更新 `ContextState` 的输入预算并重估，不复用旧模型的百分比。持久化的 `SessionEntry.context_utilization_ratio` 可继续用于离线可观测性，但**绝不**作为实时 UI 水位来源。
- **Transcript（暂缓专项）**：同一会话 JSONL 中可能出现多条 `isBoundary: false` 的 compaction 行或边界行语义重叠；合并策略与 LLM 约束待后续排查。
- **估算函数**：分层释放量与 transcript 中三字段均通过 `estimated_tokens_from_chars`（与 `estimated_token_count()` 的 **chars/4** fallback 同阶）换算；与带 API usage 的精确计数相比均为**估算**，仅保证与水位线逻辑一致。`estimate_msg_chars` 替代旧 `estimate_turn_chars`，作用于单条 `ChatMessage`。
- **防重复计入**：**L1** 在拿到 `summary_text` 时**一次性**计算 `estimatedCoveredTokensBefore` / `estimatedSummaryTokens` / `estimatedTokensSaved`，写入 `BranchSummaryEntry` 与 `CompactionResult`；**不在 L2** 再用 `estimated_token_count()` 前后差重算。**L2** `apply_boundary` **成功**后将会话 `compaction_tokens_freed` 加上 `estimated_tokens_saved`，并 `compaction_count += 1`。**L0** 作为同一成功分支的后续动作，在 ② / ⑤ / 中途守卫任一路径中将落盘与占位符释放折算为 tok 后立即计入会话累计，并在释放量非零时发射 `layer0_context_release`。**L3** 按被 drain 消息的字符累计折算 tok 后计入会话累计；**每次成功 trim 段**计 **1** 次 compact 动作（与 `compaction_count` 语义对齐），`turns_removed` 由事件给出。
- **持久化（方案 B）**：`sessions.json` 的 `SessionEntry` 持有 `compaction_count`、`compaction_tokens_freed`、`tool_result_chars_persisted`；**仅在 user turn 结束**（含可恢复错误路径）刷盘；进程在 turn 中途崩溃可能丢失本 turn 内尚未写入 store 的观测累计，详见 [session-storage.md](session-storage.md)。


