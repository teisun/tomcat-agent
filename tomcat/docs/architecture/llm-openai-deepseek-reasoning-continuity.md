# OpenAI / DeepSeek 推理续传架构方案（架构 spec）

> 适用范围：Tomcat 的 cross-turn reasoning continuity（跨 turn 推理续传）。
> 关联任务：[T2-P1-010.md](../../agents/TASK_BOARD_002/tasks/T2-P1-010.md)。
> 关联文档：[多 LLM / OpenAI 对接技术方案](llm-multiprovider-integration.md) 讲 provider 主骨架；[LLM StreamEvent → CLI/TUI 展示与 Thinking/Reasoning 协议方案](llm-stream-events-cli-pipeline.md) 讲 thinking stream / 展示 / 请求侧 thinking 参数；**本文只讲上一轮 reasoning 如何被下一轮续上**。

本文冻结三件事：

1. 为什么 **Codex 不是 OpenAI + DeepSeek 的通用主路线**。
2. 为什么 **DeepSeek 不能继续用“统一 strip thinking”** 这种粗规则。
3. 为什么最通用的设计必须是 **transcript-first（先存标准账本）**，而不是 **provider-first（先押某一家 wire）**。

先看总图：

```text
┌──────────────────────────────────────────────────────────────────────────────┐
│                           OpenAI / DeepSeek 续传总览                         │
└──────────────────────────────────────────────────────────────────────────────┘

  用户当前输入
       |
       v
  ┌──────────────────────┐
  │  本轮目标 provider   │   例如：
  │  + api + model       │   - OpenAI / responses / gpt-5
  └──────────┬───────────┘   - DeepSeek / chat.completions / deepseek-chat
             |
             v
  ┌──────────────────────────────────────────────────────────────────────────┐
  │ Provider 请求 / 流式响应                                                 │
  │ - OpenAI Responses: output_text / reasoning item / encrypted_content    │
  │ - DeepSeek Chat: content / reasoning_content / tool_calls               │
  └──────────────────────────┬───────────────────────────────────────────────┘
                             |
                             v
  ┌──────────────────────────────────────────────────────────────────────────┐
  │ Capture / Normalize Layer                                               │
  │ 把 provider 原始返回拆成 Tomcat 自己的 3 类材料                          │
  └───────────────┬──────────────────────┬──────────────────────┬───────────┘
                  |                      |                      |
                  v                      v                      v
         assistant_text           thinking_text         reasoning_continuation
         (用户可见正文)             (可读摘要/文本)         (opaque blob + source info)
                  \______________________|______________________/
                                         |
                                         v
                        ┌─────────────────────────────────┐
                        │ 标准回合账本 (TranscriptTurnRecord) │
                        │ - assistant_text                │
                        │ - thinking_text                 │
                        │ - reasoning_continuation        │
                        │ - had_tool_call                 │
                        │ - replay_requirement            │
                        └────────────────┬────────────────┘
                                         |
                                         v
                              JSONL transcript（SSoT）
                             单一事实源，先存标准账本
                                         |
                                         |
                    下一轮用户输入 + 目标 provider/model 切换
                                         |
                                         v
                        ┌─────────────────────────────────┐
                        │ ReplayPolicy                    │
                        │ + ProviderCompatProfile         │
                        └───────────────┬─────────────────┘
                                        |
           ┌────────────────────────────┼────────────────────────────┐
           |                            |                            |
           v                            v                            v
  [A] 同类 OpenAI Responses     [B] 同类 DeepSeek tool-turn    [C] 跨 provider / 不兼容
      keep opaque blob              replay reasoning_content       downgrade
      replay reasoning item         （必须回传）                  - convert fallback_text
      + optional previous_response_id                               - 或 strip 到可见历史
           \____________________________|____________________________/
                                        |
                                        v
                           下一轮目标 provider 请求体
                                        |
                                        v
                              会话继续，不因切模型断掉


  旁路说明：
  - assistant_text / thinking_text 可以给 UI / transcript / 审计使用
  - reasoning_continuation 主要给下一轮 replay，用于机器续推理
  - previous_response_id 只是 OpenAI 优化，不是主账本
```

看图顺序：

1. **上半部分**：先看 provider 原始返回，Tomcat 会把它拆成“正文 / 可读 thinking / 黑盒续传材料”三层。
2. **中间**：这三层统一收进 **标准回合账本**（`TranscriptTurnRecord`），再写进 JSONL transcript；这就是 shared transcript / SSoT。
3. **下半部分**：到下一轮发请求前，不是直接把历史原样塞回去，而是先经过 `ReplayPolicy`。
4. **最右三叉**：同类 provider 走高保真 replay；DeepSeek tool-turn 走强制回传；跨 provider 就 downgrade，优先保会话不断。

说人话：这整套方案的核心就是 **“先把历史存成 Tomcat 自己的标准账本，再在真正发请求时按目标模型的规矩翻译出去”**。

---

## 1. 术语统一


| 术语                           | 语义                                                                                                       | 数据载体                                                                                                     | 行为约束                                                                                                    | 说人话                                          |
| ---------------------------- | -------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------- | -------------------------------------------- |
| `标准回合账本`（`TranscriptTurnRecord`） | Tomcat 内部的标准 assistant turn 语义单元：正文、thinking 文本、续传材料、tool-call 元数据分开存。                                   | 建议落在 `src/core/llm/types.rs`，持久化进 `src/core/session/transcript.rs` 的 `MessageEntry.message`。             | 它是 **跨 provider 的单一事实源**；任何 `previous_response_id`、`reasoning_content`、`thinking_signature` 都只能视为它的派生物。 | 先记一本 Tomcat 自己看得懂的账本，再翻译给 OpenAI 或 DeepSeek。 |
| `thinking_text`              | 可以被人类读懂、也可以在降级时作为文本 continuity 使用的 thinking 摘要或可见 thinking 文本。                                           | `TranscriptTurnRecord` 的可选字段；来源可为 `response.reasoning_summary_`*、`reasoning_content`、或流式 thinking 汇总。 | **不能**拿它替代 provider 私有 blob；它只做展示、审计、best-effort downgrade。                                             | 给人和跨模型兜底看的“说得明白”的续传材料。                       |
| `reasoning_continuation`     | 供同类 provider / API / model 继续推理的 opaque continuity blob。                                                 | `TranscriptTurnRecord.reasoning_continuation`；内部可包 `serde_json::Value` + provider/api/model 元数据。      | 默认视为 **机器读物**；只在兼容 profile 明确允许时回放，禁止原样跨 provider 硬塞。                                                   | 一份黑盒续传材料，Tomcat 负责存着和按规矩回放。                  |
| `ProviderCompatProfile`      | 描述某个 `(provider, api, model family)` 如何 capture / replay / strip / downgrade thinking 的规则卡。              | 建议新增 `src/core/llm/replay_policy.rs`。                                                                    | 粒度必须到 **provider + api + model family**，不能只按“是不是 DeepSeek”一刀切。                                          | 每家模型各自的使用说明卡。                                |
| `ReplayPolicy`               | 根据 `TranscriptTurnRecord` 与目标 `ProviderCompatProfile` 计算 `keep / strip / convert / downgrade` 的出站决策器。 | 建议新增 `src/core/llm/replay_policy.rs`，由 provider 出站前统一调用。                                                 | 它只决定“**历史如何重放**”，不负责 thinking effort 请求参数。                                                              | 发下一轮之前，决定历史里的续传材料到底带不带、怎么带。                  |
| `GracefulDowngrade`          | 当目标 provider 不能消费来源 blob 时，把 continuity 退化为安全文本或仅保留可见历史，而不是请求失败。                                         | `ReplayPolicy` 的一条结果分支。                                                                                  | 绝不把不兼容私有字段原样下发；优先保住会话语义，再谈 continuity 完整度。                                                              | 续传降级可以，别把会话打断。                               |
| `tool_call_sensitive_replay` | 一类 profile 规则：是否发生过 tool call，会影响 thinking/reasoning 是否必须重放。                                             | `TranscriptTurnRecord` 的 `had_tool_call` / `replay_requirement` 元数据。                                  | DeepSeek thinking mode 属于此类；规则必须按 turn shape 判定。                                                        | 有些模型只有“调过工具的那轮”才必须带 reasoning 回去。            |
| `previous_response_id`       | OpenAI Responses 的会话优化锚点，可让下一轮引用上一轮 response。                                                            | `reasoning_continuation.provider_refs.openai_response_id`（建议形状）。                                         | **只能**作为 OpenAI 专属优化；**且要求 prior 请求 `store=true`**，与 Tomcat 当前 `store=false` 主线互斥，不能作为跨 provider continuity 的主设计。 | OpenAI 自家的高速路入口；要走它得切回 `store=true`，不是通用账本。                      |
| `think scrubber`             | 把 `<think>` / `<reasoning>` 这类错误混进正文的思考块从可见流里剔除的状态机。                                                     | 可借鉴 `hermes-agent/agent/think_scrubber.py`。                                                              | 它只解决“别把脑内草稿漏给用户”，**不**等于 continuity replay。                                                             | 正文保洁层。                                       |


说人话：本文里最重要的不是某个字段名，而是这条边界: **“用户看到的回答”**、**“给下一轮续推理的材料”**、**“某厂商私有的 wire 优化”** 必须拆开。

---

## 2. 竞品 / 选型对比（调研）

### 2.1 官方协议事实基线


| 来源                                                                                   | 官方事实                                                                                                                                                    | 对架构的直接约束                                                                                           | 说人话                                        |
| ------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------- | ------------------------------------------ |
| [OpenAI Responses API](https://platform.openai.com/docs/api-reference/responses)     | reasoning item 可返回 `encrypted_content`；官方说明该字段用于 `store=false` 一类 stateless multi-turn continuity；同时提供 `previous_response_id` 作为 conversation state 优化。 | OpenAI 路径可以同时支持 **显式 replay reasoning items** 与 `**previous_response_id` 优化**，但这两者都属于 OpenAI 专属能力。 | OpenAI 既给你“自己回放历史”的能力，也给你“让服务器帮记上一轮”的快捷方式。 |
| [DeepSeek `deepseek-reasoner`](https://api-docs.deepseek.com/guides/reasoning_model) | 普通多轮里如果把旧 `reasoning_content` 带回输入，会 `400`；且该模型不支持 function calling。                                                                                    | 不能把 “DeepSeek = 总要回放 reasoning_content” 当成统一结论；至少要按模型家族细分 profile。                                 | 不是所有 DeepSeek 模型都爱吃旧 reasoning。            |
| [DeepSeek Thinking Mode](https://api-docs.deepseek.com/guides/thinking_mode)         | 若两次 `user` 之间 **没有** tool call，中间 assistant 的 `reasoning_content` 不必参与后续上下文；若 **发生过** tool call，则之后所有相关 turn 都必须继续回传该 `reasoning_content`，否则 `400`。     | Tomcat 的 replay 规则必须看 **turn 是否发生过 tool call**；统一 strip 或统一 keep 都不正确。                             | DeepSeek 的关键不是“是不是 thinking”，而是“这轮有没有走工具”。 |


说人话：OpenAI 给的是“可以回放 opaque reasoning item”；DeepSeek 给的是“某些 turn 必须回放 `reasoning_content`，某些 turn 反而不能乱带”。所以主方案不能押单一 wire。

### 2.2 六份实现证据表


| 来源            | 关键文件                                                                                                                                                                                       | 这份实现证明了什么                                                                                                                                                                   | 我们借鉴的点                                                                                                                  | 说人话                                          |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- | -------------------------------------------- |
| Tomcat（现状）    | `src/core/llm/openai_responses/mod.rs`、`src/core/llm/openai_responses/payload.rs`、`src/core/session/transcript.rs`、`src/core/session/manager/context.rs`、`src/core/llm/thinking_policy.rs` | Responses 请求固定 `store=false`；payload 只回放 `output_text/function_call`；`ThinkingTrace` 不参与 hydrate；`should_strip_on_resend()` 仍是粗粒度布尔策略。                                      | 当前已有 `StreamEvent::Thinking`、Responses stream parser、JSONL transcript 基座；缺的是 transcript continuity 字段与出站 replay policy。 | 地基有了，但“怎么把上一轮 reasoning 重新带出去”还没钉死。          |
| Codex         | `codex-rs/core/src/client.rs`                                                                                                                                                              | 以 OpenAI Responses 为中心，维护 turn-scoped websocket、sticky routing token、`previous_response_id` 与增量请求优化。                                                                        | 说明 `previous_response_id` / websocket prewarm 很强，但它们是 **OpenAI Responses 深度优化**。                                        | Codex 像 OpenAI 专用高速路，跑得快，但不是通用道路标准。          |
| Hermes        | `hermes-agent/agent/codex_responses_adapter.py`、`hermes-agent/agent/transports/codex.py`、`hermes-agent/agent/think_scrubber.py`                                                            | chat-style messages 可先 normalize 成 Responses input；显式保存/回放 `codex_reasoning_items`、`codex_message_items`；出站前可按 transport 清洗字段；流式 `<think>` 可被状态机 scrub。                     | 借它的 **normalize / explicit replay / transport strip / think scrubber** 四件套，不照搬整套 transport 胶水层。                         | Hermes 最值钱的是“翻译层 + 续传层 + 正文保洁层”的细节。          |
| pi-mono       | `packages/ai/src/providers/openai-responses-shared.ts`、`packages/ai/src/providers/transform-messages.ts`                                                                                   | transcript 是 provider-agnostic 的消息 AST；同模型保留 thinking signature，跨模型丢 redacted thinking 或转纯文本；必要时合成 tool result 维持 replay 合法性。                                               | 借它的 **transcript-first + cross-model sanitize/downgrade** 思路。                                                           | 先存标准消息，再看目标模型能吃多少。                           |
| pi_agent_rust | `src/providers/openai_responses.rs`                                                                                                                                                        | Rust 里把对话翻成 Responses `input`、解析 `response.reasoning_`* 流、把 thinking 当一等事件；同时把 OpenAI Responses 的请求/响应形状钉在 provider 内。                                                      | 借它的 **Responses 输入构造与 thinking streaming 状态机**。                                                                         | 证明 transcript-first 也能在 Rust 里落得很干净。         |
| OpenClaw      | `src/agents/openai-completions-compat.ts`、`src/agents/transcript-policy.ts`                                                                                                                | provider / endpoint / model family 会生成不同 compat defaults；可声明 `requiresReasoningContentOnAssistantMessages`、`dropReasoningFromHistory`、`preserveSignatures` 等 replay policy。 | 借它的 **ProviderCompatProfile / TranscriptPolicy** 这类设计颗粒度。                                                               | 规则不是“按厂商”，而是“按厂商 + API + 模型家族 + turn shape”。 |


### 2.3 Hermes 设计拆解

Hermes 不是本文的主路线，但它提供了最值得 Tomcat 借鉴的一组客户端 replay 细节。

#### 专业术语版

- `normalize`：把 provider 原始响应先转成内部一致的消息/块结构，再决定哪些部分可见、哪些部分只用于 replay。
- `explicit replay`：把 `codex_reasoning_items`、`codex_message_items`、`reasoning_details` 这类 provider_data 显式保存在 assistant message 上，并在下一轮重新放回 Responses `input`。
- `transport strip`：在真正发请求前，按目标 transport 清掉不兼容字段，避免 OpenAI 形 wire 被 DeepSeek / xAI / 代理网关的兼容差异打挂。
- `think scrubber`：对流式正文做增量状态机过滤，避免 `<think>` 标签被拆分到多个 delta 时漏到用户界面。

#### 说人话版

Hermes 本质上像三层叠加：

1. **翻译层**：把各家 provider 的“原始回包”翻成内部统一消息。
2. **续传层**：把真正能帮下一轮续推理的东西单独存起来，下轮按需回放。
3. **正文保洁层**：如果模型把脑内草稿混进正文，就在展示前清掉。

#### ASCII 图

```text
Provider Response
      |
      +--> reasoning item ---------> normalize ---------> stored reasoning fields
      |                                                       |
      |                                                       +--> next turn replay
      |
      +--> assistant visible text --> think scrubber --> clean assistant text
                                                          |
                                                          +--> user-visible output
```

#### 为什么不整套照搬 Hermes


| 项            | 不直接照搬的原因                                                                                               | 说人话                      |
| ------------ | ------------------------------------------------------------------------------------------------------ | ------------------------ |
| Transport 体系 | Tomcat 当前不是 Hermes 那种 provider transport 总线；整套搬入会把改动面扩到主循环与 transport 抽象。                              | 第一版会做重。                  |
| 消息基座         | Tomcat 已有 `ChatMessage` / JSONL transcript，没必要先改成 Hermes 的整套内部消息层。                                     | 不用为了学到 replay 技巧把整个地基换掉。 |
| 我们真正需要的部分    | normalize、explicit replay、transport strip、think scrubber 都可以作为局部借鉴落到 replay policy 和 provider adapter。 | 抄精华，不抄体型。                |

这两行说的是 **两层完全不同的东西**：

- **Transport 体系**：回答“**谁负责把内部消息翻成具体 provider 请求**？”这是调用链 / 模块分层问题。
- **消息基座**：回答“**Tomcat 内部到底把一轮对话存成什么样**？”这是数据模型 / transcript 真理来源问题。

```text
一、Transport 体系（谁负责发请求）

Hermes（整套）

Agent Loop
   |
   v
ProviderTransport
   |
   +--> convert_messages()
   +--> convert_tools()
   +--> build_kwargs()
   +--> normalize_response()
   |
   v
OpenAI / Codex / xAI / ...


Tomcat（当前）

Agent Loop
   |
   v
LlmProvider
   |
   +--> openai.rs
   +--> openai_responses/*
   |
   v
HTTP / SSE / NDJSON


区别：
- Hermes 多了一层统一 Transport 总线。
- Tomcat 现在是 provider impl 直接吃 `ChatRequest`，直接发 HTTP。
- 所以“整套照搬 Hermes”会先动调用链和模块边界。
```

```text
二、消息基座（内部把历史存成什么）

Hermes（如果整套照搬它的消息层）

provider raw response
      |
      v
normalized internal message
      |
      +--> reasoning fields
      +--> replay-only fields
      +--> visible assistant text


Tomcat（我们现在 / 建议路线）

ChatMessage + JSONL transcript
          |
          v
标准回合账本 (TranscriptTurnRecord)
   +--> assistant_text
   +--> thinking_text
   +--> reasoning_continuation
   +--> had_tool_call / replay_requirement


区别：
- Hermes 这层是在说“内部消息结构怎么重新定义”。
- Tomcat 这层已经有自己的 `ChatMessage` + transcript 基座了。
- 所以没必要为了学 replay 技巧，把整套内部消息模型也一起换掉。
```

说人话：**Transport 体系**像“道路和收费站怎么修”；**消息基座**像“货物本身按什么规格装箱”。前者改的是调用链，后者改的是内部账本。本文拒绝的是“为了借 Hermes 的 replay 细节，把这两层一起重做”。


### 2.4 为什么 Codex 不是通用主路线

1. Codex 的强项是 **Responses-only 深优化**：`previous_response_id`、websocket prewarm、sticky routing token，本质上都围绕 OpenAI Responses 设计。
2. DeepSeek 官方当前主路径仍是 **`/v1/chat/completions` + `reasoning_content`**，没有 OpenAI `/responses` 的通用等价面。
3. 如果把 continuity 主设计建在 `previous_response_id` 上，切到 DeepSeek 时就会失去主账本，只剩 provider 私有优化残片。

说人话：Codex 很适合做 OpenAI 路线的“快车道优化”，但不适合做 OpenAI + DeepSeek 的“道路标准”。

### 2.5 为什么主路线必须是 transcript-first


| 结论                                      | 理由                                                             | 说人话               |
| --------------------------------------- | -------------------------------------------------------------- | ----------------- |
| 共享 transcript 必须先于 provider wire 存在     | 只有 transcript-first，才能在 provider 切换时保留一份独立于 wire 的历史真相。        | 先有标准账本，后有各家翻译。    |
| continuity 材料必须拆成“可读文本”和“opaque blob”两层 | 只有这样，才能既支持同 provider 高保真 replay，又支持跨 provider 降级。              | 既要机器能续，也要换模型时不爆。  |
| replay 规则必须是 profile 驱动                 | DeepSeek、OpenAI、未来 Anthropic 的 thinking ingest / egress 规则不一样。 | 别再用一个全局布尔开关赌所有厂商。 |


---

## 3. 目标与设计原则

### 3.1 目标表


| 目标                                                            | 观察指标（落地后用户可感知）                                                                                             | 说人话                            |
| ------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- | ------------------------------ |
| G1 同一份 transcript 可在 OpenAI 与 DeepSeek 间继续会话                  | 切换 provider 后，历史回放不报协议错误，会话不断裂。                                                                            | 换模型可以降级，但不能断片。                 |
| G2 OpenAI Responses 在 `store=false` 下仍可做 reasoning continuity | transcript 中保存的 reasoning item / `encrypted_content` 能在下一轮显式 replay。                                       | 不靠服务器记忆，也能续上。                  |
| G3 DeepSeek thinking mode 工具回合不再误用统一 strip                    | tool-call turn 后续请求会保留必需的 `reasoning_content`；无 tool turn 不盲目回放。                                           | DeepSeek 该带的带，不该带的别带。          |
| G4 跨 provider 切换时不传私有脏字段                                      | OpenAI 的 reasoning item 不会原样出现在 DeepSeek wire；DeepSeek 的 `reasoning_content` 也不会被塞进 OpenAI Responses item。 | 私货别串线。                         |
| G5 展示、持久化、上行 replay 三条管线解耦                                    | 关闭 thinking 展示不会影响 continuity；是否持久化 thinking 文本也不改变 replay 正确性。                                            | 别把“能不能看见 thinking”和“能不能续推理”绑死。 |
| G6 老 transcript 可平滑回放                                         | 历史 JSONL 没有 continuity 字段时，仍可按可见消息正常聊天。                                                                    | 旧会话不需要迁移才能继续用。                 |


### 3.2 非目标


| 非目标                                            | 推给                                                                     | 说人话                                  |
| ---------------------------------------------- | ---------------------------------------------------------------------- | ------------------------------------ |
| Anthropic / Gemini / Doubao 全家首期落地             | 后续 provider continuity 任务                                              | 本文先把 OpenAI + DeepSeek 钉死。           |
| 用 `previous_response_id` 取代 transcript 作为主会话状态 | 本文明确拒绝                                                                 | 服务器记忆不是主账本。                          |
| 终端 thinking 展示、折叠、CLI/TUI 交互                   | [llm-stream-events-cli-pipeline.md](llm-stream-events-cli-pipeline.md) | 这篇不讲“怎么看 thinking”，只讲“怎么续 thinking”。 |
| 全量移植 Hermes transport 抽象                       | 后续若 provider 层重构再评估                                                    | 先借细节，不重做主循环。                         |


---

## 4. 落地选型与实施（已定稿）

### 4.1 落地选型决策表


| 维度                             | 关切                                                    | 决策                                                                                               | 取自                                                                                                                                                              | 入选理由                                                                                                      | 未入选 + 拒因                                                                                                                     | 说人话                                   |
| ------------------------------ | ----------------------------------------------------- | ------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- | ------------------------------------- |
| R1 主真理来源                       | continuity 的单一事实源到底是什么                                | **采用 transcript-first：`TranscriptTurnRecord` 为主真理，provider session id / response id 只做派生引用。** | `tomcat/src/core/session/transcript.rs`；`tomcat/src/core/session/manager/context.rs`；`pi-mono/packages/ai/src/providers/transform-messages.ts`                  | 设计：先在 Tomcat 内保存 provider-agnostic turn，再在 provider 出站时翻译；理由：只有这样才能支持跨 provider 切换与 legacy transcript 回放。 | 未入选：Codex 风格把 continuity 重心放在 `previous_response_id` / websocket session；拒因：DeepSeek 不成立，且一旦离开 Responses 就丢主账本。              | 主账本必须在 Tomcat 手里。                     |
| R2 continuity 载体               | 续传材料是只存文本、只存 blob，还是两层分离                              | **采用“`thinking_text` + `reasoning_continuation` 分层存储”。**                                         | `tomcat/src/core/llm/openai_responses/stream.rs`；`hermes-agent/agent/codex_responses_adapter.py`；`pi-mono/packages/ai/src/providers/openai-responses-shared.ts` | 设计：可读 thinking 与 opaque blob 分开；理由：同 provider 可高保真 replay，跨 provider 时仍有安全 downgrade 材料。                  | 未入选：只存 thinking 文本；拒因：OpenAI Responses 的 `encrypted_content` / reasoning item 无法还原。未入选：只存 opaque blob；拒因：跨 provider 切换时无法兜底。 | 一份给机器，一份给人和降级兜底。                      |
| R3 replay 规则粒度                 | 是不是一个全局 `strip_on_resend` 开关就够                        | **采用 `ProviderCompatProfile + ReplayPolicy`，粒度到 `(provider, api, model family, turn shape)`。**   | `tomcat/src/core/llm/thinking_policy.rs`；`openclaw/src/agents/transcript-policy.ts`；`openclaw/src/agents/openai-completions-compat.ts`                          | 设计：把 capture / replay / strip / downgrade 规则集中在 profile + policy；理由：DeepSeek 与 OpenAI 的差异已经证明全局布尔开关过粗。    | 未入选：继续扩展 `should_strip_on_resend()` 一类全局布尔逻辑；拒因：无法表达“DeepSeek 仅 tool-call turn 必须 replay”。                                   | 规则必须按对象分层，不是一个开关打天下。                  |
| R4 OpenAI Responses continuity | OpenAI 路径靠什么续推理                                       | **主线：`store=false` + 显式 replay reasoning item（含 `encrypted_content`，请求需带 `include=["reasoning.encrypted_content"]`）；`previous_response_id` 是另一条互斥路径，要求切回 `store=true`，默认不启用。**             | `tomcat/src/core/llm/openai_responses/payload.rs`；`pi_agent_rust/src/providers/openai_responses.rs`；`hermes-agent/agent/transports/codex.py`                    | 设计：显式 replay 是 stateless continuity 主链；理由：这样即使 `store=false` 也不丢跨 turn reasoning，且 transcript 是单一事实源。   | 未入选：只保留 `previous_response_id`；拒因：要求 `store=true`，与当前 stateless 主线冲突，也无法覆盖 DeepSeek。                                                      | OpenAI 这边先保证“自己带历史也能续上”，快车道（`previous_response_id`）默认不开。        |
| R5 DeepSeek continuity         | DeepSeek 历史里的 `reasoning_content` 怎么处理                | **采用 tool-call-sensitive replay：无 tool turn 默认不带；tool-call turn 后续请求必须回传。**                      | `tomcat/src/core/llm/openai.rs`；DeepSeek Thinking Mode 官方文档；`openclaw/src/agents/openai-completions-compat.ts`                                                  | 设计：把 `had_tool_call` 与 `replay_requirement` 写进 transcript metadata；理由：这正是 DeepSeek 官方协议的硬约束。              | 未入选：统一 strip；拒因：tool-call turn 会 400。未入选：统一 keep；拒因：`deepseek-reasoner` 普通多轮会 400，且无意义放大上下文。                                 | DeepSeek 不是“总带”也不是“总删”，而是“看这轮有没有调工具”。 |
| R6 跨 provider 切换               | OpenAI blob 切到 DeepSeek、或 DeepSeek blob 切到 OpenAI 怎么办 | **采用 `GracefulDowngrade`：不兼容 blob 一律不透传；优先使用 `thinking_text`，否则退到可见 assistant 历史。**              | `tomcat/src/core/session/transcript.rs`；`pi-mono/packages/ai/src/providers/transform-messages.ts`；`openclaw/src/agents/transcript-policy.ts`                    | 设计：兼容时 keep opaque，不兼容时 convert 或 strip；理由：目标是保证会话不断而不是强行互通私有 blob。                                       | 未入选：跨 provider 原样透传私有字段；拒因：请求高概率 4xx，且把 provider 私货污染共享 transcript。                                                          | 先保住会话，续传能保多少算多少。                      |
| R7 泄漏控制                        | continuity 做强后，怎么避免 CoT 泄漏到正文                         | **采用 Hermes 风格的 `think scrubber` 与 transport strip，但保持它们是 replay 辅助层而非主设计。**                     | `tomcat/src/core/llm/openai_responses/stream.rs`；`hermes-agent/agent/think_scrubber.py`                                                                         | 设计：把“用户能看到什么”与“下一轮能回放什么”拆开；理由：同一段 thinking 可能既不该给用户看，也不能直接丢掉。                                             | 未入选：靠 prompt 约束模型别输出 `<think>`；拒因：流式边界一旦拆段，仍可能泄漏。                                                                            | 有了续传之后，更要把“别漏给用户”单独守住。                |


### 4.2 实施点（已闭环定义）


| 实施点    | 交付范围（含交付物）                                                                                           | 主要代码落点（含落地点）                                                                                                                | 验收锚点（示例）                                                | 说人话                         |
| ------ | ---------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------- | --------------------------- |
| **P1** | `TranscriptTurnRecord` / `reasoning_continuation` / `ProviderCompatProfile` / `ReplayPolicy` 类型定稿 | `src/core/llm/types.rs`、`src/core/session/transcript.rs`、`src/core/llm/replay_policy.rs`（建议新增）                              | `transcript_roundtrip_preserves_reasoning_continuation` | 先把账本和规则卡定义好。                |
| **P2** | OpenAI Responses capture + replay：`response.reasoning_`*、`encrypted_content`、`response_id`           | `src/core/llm/openai_responses/stream.rs`、`src/core/llm/openai_responses/payload.rs`、`src/core/llm/openai_responses/mod.rs` | `openai_responses_roundtrip_replays_reasoning_items`    | 把 OpenAI 这条链路先闭环。           |
| **P3** | DeepSeek `chat.completions` tool-call-sensitive replay（当前经 OpenAI-compatible adapter 接入）         | `src/core/llm/openai.rs`、`src/core/llm/replay_policy.rs`                                                                    | `deepseek_tool_turn_replays_reasoning_content`          | 把 DeepSeek “该带的带、该删的删”落成代码。 |
| **P4** | cross-provider downgrade + legacy transcript fallback                                                | `src/core/session/manager/context.rs`、`src/core/llm/replay_policy.rs`                                                       | `cross_provider_downgrade_keeps_semantic_history`       | 切换模型时别断片。                   |
| **P5** | Hermes 风格 transport strip / think scrubber 补位                                                        | `src/core/llm/openai.rs` / `openai_responses/`* 出站层；必要时新增 scrubber helper                                                   | `streaming_think_scrubber_hides_split_tags`             | 把脏字段和 `<think>` 漏出问题堵上。     |
| **P6** | 文档边界、相邻文档回链、测试矩阵同步                                                                                   | 本文、`llm-multiprovider-integration.md`、`host-core-layer.md`                                                                  | 文档交叉引用 grep / review                                    | 把“这篇讲什么、不讲什么”写清楚。           |


#### 4.2.1 核心抽象定稿

建议的 Rust 语义形状：

```rust
struct TranscriptTurnRecord {
    assistant_text: String,
    thinking_text: Option<String>,
    reasoning_continuation: Option<ReasoningContinuation>,
    continuity: ContinuityMetadata,
}

struct ReasoningContinuation {
    source_provider: String,
    source_api: String,
    source_model: String,
    format: ReasoningFormat,
    opaque_payload: serde_json::Value,
    fallback_text: Option<String>,
    provider_refs: ProviderRefs,
}

struct ContinuityMetadata {
    had_tool_call: bool,
    replay_requirement: ReplayRequirement,
}
```

说人话：**标准回合账本**（`TranscriptTurnRecord`）不是某一家 wire 的 JSON，它只是把“上一轮到底发生了什么”拆成 Tomcat 自己能理解的几块。

#### 4.2.2 OpenAI Responses 路线

OpenAI Responses 实际是**两条互斥**的 continuity 路径，**不能叠加**：

| 路径 | 请求形态 | 历史来源 | Tomcat 是否默认走 |
|------|----------|----------|------------------|
| **A. stateless replay**（主线） | `store=false` + `include=["reasoning.encrypted_content"]` | client 显式回放 reasoning item | **是** |
| **B. server-stored 续传**（可选） | `store=true` + `previous_response_id` | 服务端历史 | 否，需配置开启并整体切到 `store=true` |

落到本方案：

- capture：请求侧带 `include=["reasoning.encrypted_content"]`，从 `response.reasoning_*` 事件收集 `thinking_text`，从完整 reasoning item 里收集 `encrypted_content` 或等价 opaque payload。
- persist：把 `response_id` 作为 `provider_refs.openai_response_id` 存进 `reasoning_continuation`，仅供路径 B 使用；它**不**取代 transcript。
- replay（路径 A）：把存下来的 reasoning item 显式回放进 `input`。
- replay（路径 B）：请求改为 `store=true` 且不重复 replay 历史 reasoning item，只附带 `previous_response_id`；若该 id 失效，warning 并退化为路径 A 重试。
- fallback：若 replay blob 无效（路径 A）或 id 失效（路径 B），降级到仅可见历史。

#### 4.2.3 DeepSeek 路线

- 先把边界说清楚：**本期不要求仓库先有独立 `provider=deepseek`**。当前规划是把 `src/core/llm/openai.rs` 视为 **OpenAI-compatible Chat Completions adapter**，通过 `provider="openai"` + `api_base="https://api.deepseek.com"` + `api_key_env="DEEPSEEK_API_KEY"` + `default_model="deepseek-chat"`（必要时 `thinking.format="deepseek"`）去接 DeepSeek。
- 这样做的原因是：本期关注的是 continuity capture / replay / downgrade 语义，而不是为每个“类 OpenAI”厂商复制一份 transport/provider 骨架。只要 wire 仍兼容 OpenAI Chat Completions，优先复用现有 adapter。
- capture：把 assistant 的 `reasoning_content` 存入 `reasoning_continuation`，并记录该 turn 是否发生过 tool call。
- replay：若 profile 判定“无 tool turn”，默认 strip；若判定“tool-call turn”，则在后续请求中持续回传 `reasoning_content`。
- model split：`deepseek-reasoner` 与 `deepseek-chat thinking mode`（V3.1+）必须是两个 profile，不能混用——前者**不能**回放历史 `reasoning_content`，后者按 tool-call 决定是否回放。
- 只有当某家 OpenAI-compatible 后端在请求/响应字段、流式事件、错误模型、重试策略、专属能力或用户配置语义上明显分叉时，才考虑拆出新的 provider id / 独立 provider 实现。

#### 4.2.4 跨 provider graceful downgrade

- 优先级 1：目标 profile 兼容来源 blob，`keep opaque`。
- 优先级 2：目标 profile 不兼容，但 transcript 有 `thinking_text`，`convert to text continuity`。
- 优先级 3：两者都没有，`strip opaque`，仅保留可见 assistant / tool 历史。

说人话：跨 provider 切换时，Tomcat 追求的是“继续聊下去”，不是“强求私有续传材料百分百互通”。

#### 4.2.5 Hermes 借鉴边界

Tomcat 第一版只吸收 Hermes 的四个局部能力：

1. normalize 到内部统一 turn；
2. explicit replay provider_data；
3. transport strip；
4. think scrubber。

**不**把 provider transport 总线与主循环抽象整体改造成 Hermes 风格。

---

## 5. 协议（入参 / 出参 / Schema）

### 5.1 标准回合账本（`TranscriptTurnRecord`）扩展字段

单一事实源建议：`src/core/llm/types.rs` 的消息类型定义；`src/core/session/transcript.rs` 负责 JSONL 持久化。


| 字段                                       | JSON 类型                   | 必填  | 默认值     | 适用场景                        | 说明                                                                  | 说人话              |
| ---------------------------------------- | ------------------------- | --- | ------- | --------------------------- | ------------------------------------------------------------------- | ---------------- |
| `assistant_text`                         | `string`                  | 是   | 无       | 所有 assistant turn           | 用户可见正文。                                                             | 模型正式回答。          |
| `thinking_text`                          | `string | null`           | 否   | `null`  | 可显示 thinking 或 downgrade 场景 | 安全文本版 continuity；可来自 summary 或可见 reasoning 文本。                      | 给人读、给跨模型兜底。      |
| `reasoning_continuation.source_provider` | `string`                  | 否   | 无       | 存在 opaque blob 时            | 来源 provider id，例如 `openai`、`deepseek`。                              | 这份黑盒是谁家的。        |
| `reasoning_continuation.source_api`      | `string`                  | 否   | 无       | 同上                          | 来源 API family，例如 `responses`、`chat_completions`。                    | 这份黑盒是走哪种接口产出的。   |
| `reasoning_continuation.source_model`    | `string`                  | 否   | 无       | 同上                          | 来源 model family / id。                                               | 哪个模型生成了它。        |
| `reasoning_continuation.format`          | `string enum`             | 否   | 无       | 同上                          | 例如 `openai_responses_reasoning_items`、`deepseek_reasoning_content`。 | 标清黑盒长什么样。        |
| `reasoning_continuation.opaque_payload`  | `object | array | string` | 否   | 无       | 同上                          | provider 私有 replay 材料；默认不做跨 provider 解释。                            | 真正给模型续推理的黑盒本体。   |
| `reasoning_continuation.fallback_text`   | `string | null`           | 否   | `null`  | 需要 graceful downgrade 时     | 与 blob 对应的安全文本版 continuity。                                         | 黑盒吃不下时的文字备胎。     |
| `reasoning_continuation.provider_refs`   | `object | null`           | 否   | `null`  | provider 私有优化               | 例如 `openai_response_id`。                                            | 厂商专属快捷方式。        |
| `continuity.had_tool_call`               | `bool`                    | 是   | `false` | 所有 assistant turn           | 该 turn 是否发生过 tool call。                                             | 这轮有没有调工具。        |
| `continuity.replay_requirement`          | `string enum`             | 是   | `never` | 所有 assistant turn           | `never` / `same_profile_optional` / `same_profile_required`。        | 这轮的续传材料是不是必须带回去。 |


### 5.2 参考 JSON 形状

#### 5.2.1 transcript 中的 assistant turn（示意）

```jsonc
{
  "role": "assistant",
  "assistant_text": "杭州明天多云，7~13°C。",
  "thinking_text": "先取日期，再取天气，最后组织答复。",
  "reasoning_continuation": {
    "source_provider": "deepseek",
    "source_api": "chat_completions",
    "source_model": "deepseek-chat",
    "format": "deepseek_reasoning_content",
    "opaque_payload": {
      "reasoning_content": "Today is 2026-04-19, so tomorrow is 2026-04-20..."
    },
    "fallback_text": "先取日期，再取天气，最后组织答复。"
  },
  "continuity": {
    "had_tool_call": true,
    "replay_requirement": "same_profile_required"
  }
}
```

#### 5.2.2 OpenAI Responses 出站 replay（示意）

**路径 A：`store=false` + 显式 replay**（当前主线）

```jsonc
{
  "model": "gpt-5",
  "input": [
    { "type": "reasoning", "encrypted_content": "..." },
    { "type": "message", "role": "assistant", "content": [{ "type": "output_text", "text": "..." }] },
    { "type": "function_call", "call_id": "call_123", "name": "read", "arguments": "{\"path\":\"src/main.rs\"}" }
  ],
  "store": false,
  "include": ["reasoning.encrypted_content"]
}
```

**路径 B：`store=true` + `previous_response_id`**（可选优化；与路径 A 互斥）

```jsonc
{
  "model": "gpt-5",
  "input": [
    { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "继续上面的任务" }] }
  ],
  "store": true,
  "previous_response_id": "resp_abc123"
}
```

#### 5.2.3 DeepSeek 出站 replay（tool-call turn）

```jsonc
[
  { "role": "user", "content": "How's the weather in Hangzhou tomorrow?" },
  {
    "role": "assistant",
    "content": "",
    "reasoning_content": "The user is asking about the weather in Hangzhou tomorrow...",
    "tool_calls": [
      {
        "id": "call_00_kw66qNnNto11bSfJVIdlV5Oo",
        "type": "function",
        "function": { "name": "get_date", "arguments": "{}" }
      }
    ]
  },
  { "role": "tool", "tool_call_id": "call_00_kw66qNnNto11bSfJVIdlV5Oo", "content": "2026-04-19" }
]
```

### 5.3 `ProviderCompatProfile` 最小字段


| 字段                          | JSON 类型       | 必填  | 默认值                    | 适用场景                      | 说明                                                            | 说人话                |
| --------------------------- | ------------- | --- | ---------------------- | ------------------------- | ------------------------------------------------------------- | ------------------ |
| `profile_id`                | `string`      | 是   | 无                      | 所有 profile                | 例如 `openai.responses.default`、`deepseek.chat.tool_sensitive`。 | 规则卡名字。             |
| `capture_mode`              | `string enum` | 是   | 无                      | 所有 profile                | `opaque_items` / `reasoning_content` / `none`。                | 这家该抓什么。            |
| `replay_acceptance`         | `string enum` | 是   | 无                      | 所有 profile                | `same_profile_only` / `same_api_family` / `never`。            | 这家能吃谁家的黑盒。         |
| `requires_tool_turn_replay` | `bool`        | 是   | `false`                | Tool-sensitive provider   | 发生过 tool call 时是否必须回放 reasoning。                              | 有没有“调工具后必须带回去”的规矩。 |
| `supports_response_id_hint` | `bool`        | 是   | `false`                | OpenAI 类 profile          | 是否支持 `previous_response_id` 之类优化锚点。                           | 有没有快车道。            |
| `downgrade_mode`            | `string enum` | 是   | `visible_history_only` | 跨 provider / invalid blob | `fallback_text` / `visible_history_only`。                     | 吃不下黑盒时怎么退。         |


说人话：`ProviderCompatProfile` 决定“该抓什么、能回放什么、回放失败时怎么退”；`ReplayPolicy` 只是在每次出站前按这张卡算一遍。

---

## 6. 文件职责总览（One-Glance Map）

```text
┌──────────────────────────────────────────────────────────────────────────────┐
│ src/core/llm/types.rs                                                      │
│   · 定义 ChatMessage / TranscriptTurnRecord / ReasoningContinuation      │
│   · continuity 元数据的单一事实源                                          │
├──────────────────────────────────────────────────────────────────────────────┤
│ src/core/session/transcript.rs                                             │
│   · JSONL MessageEntry 持久化 continuity 字段                              │
│   · 老 transcript 无该字段时默认 None                                      │
├──────────────────────────────────────────────────────────────────────────────┤
│ src/core/session/manager/context.rs                                        │
│   · transcript -> ChatMessage hydrate                                      │
│   · compaction / branch boundary 后仍保 continuity 元数据                   │
├──────────────────────────────────────────────────────────────────────────────┤
│ src/core/llm/replay_policy.rs   【建议新增】                                │
│   · ProviderCompatProfile                                                   │
│   · ReplayPolicy::plan(target_profile, turn) -> keep/strip/convert/...     │
│   · GracefulDowngrade                                                       │
├──────────────────────────────────────────────────────────────────────────────┤
│ src/core/llm/openai_responses/stream.rs                                    │
│   · capture response.reasoning_* / output_text                             │
│   · 汇总 thinking_text + opaque reasoning items                            │
├──────────────────────────────────────────────────────────────────────────────┤
│ src/core/llm/openai_responses/payload.rs                                   │
│   · 按 ReplayPolicy 回放 reasoning items / function calls                  │
│   · 写入 optional previous_response_id                                     │
├──────────────────────────────────────────────────────────────────────────────┤
│ src/core/llm/openai_responses/mod.rs                                       │
│   · 组 request body；默认 store=false + include=["reasoning.encrypted_content"] │
│   · use_previous_response_id 开关同时切到 store=true 分支（与默认主线互斥）       │
├──────────────────────────────────────────────────────────────────────────────┤
│ src/core/llm/openai.rs                                                     │
│   · OpenAI-compatible chat.completions 出站组装                             │
│   · 当前也承载 DeepSeek 经 api_base 复用的 reasoning_content capture/replay  │
│   · transport strip / think scrubber 接口点                                 │
├──────────────────────────────────────────────────────────────────────────────┤
│ src/core/llm/thinking_policy.rs                                            │
│   · 只保留请求侧 thinking effort / toggle 映射                              │
│   · 粗粒度 should_strip_on_resend 退位给 replay_policy                      │
├──────────────────────────────────────────────────────────────────────────────┤
│ tests                                                                       │
│   · replay policy / transcript roundtrip / OpenAI / DeepSeek fixtures      │
└──────────────────────────────────────────────────────────────────────────────┘
```

说人话：链路顺序是 **stream capture -> transcript persist -> replay policy -> provider payload**。`thinking_policy.rs` 管“这次请求要不要多想”；`replay_policy.rs` 管“上一轮的 thinking 到底怎么带回来”。

---

## 7. 调度时序（运行时图）

### 7.1 capture -> persist -> replay（同 provider）

```text
User turn
   │
   ▼
Provider stream (OpenAI Responses / DeepSeek Chat)
   │
   ├─ capture visible answer ----------┐
   ├─ capture thinking_text -----------┼--> TranscriptTurnRecord
   └─ capture reasoning_continuation --┘
                                        │
                                        ▼
                                 transcript append
                                        │
Next user turn                          ▼
   │                              hydrate history
   ▼                                    │
ReplayPolicy.plan(target profile, turns)│
   │                                    ▼
   ├─ keep opaque -----------------> provider payload
   ├─ convert fallback_text -------> provider payload
   └─ strip to visible history ----> provider payload
```

### 7.2 OpenAI -> DeepSeek 切换

```text
OpenAI transcript turn
  assistant_text + thinking_text + openai reasoning blob
                    │
                    ▼
         ReplayPolicy(target = DeepSeek)
                    │
        profile mismatch on opaque blob
                    │
        ├─ fallback_text exists  -> convert to text continuity
        └─ fallback_text absent  -> strip opaque, keep visible history
                    │
                    ▼
          DeepSeek chat.completions request
```

### 7.3 DeepSeek tool-call turn -> 后续 turn

```text
DeepSeek assistant(tool call)
  content + reasoning_content + tool_calls
                    │
                    ▼
 TranscriptTurnRecord.continuity:
   had_tool_call = true
   replay_requirement = same_profile_required
                    │
                    ▼
Later user turn (same DeepSeek profile)
                    │
                    ▼
ReplayPolicy => keep reasoning_content on every subsequent request
```

---

## 8. 状态机

```text
┌──────────────┐ 无 continuity blob       ┌──────────────┐
│   NewTurn    │─────────────────────────▶│ VisibleOnly  │
└──────┬───────┘                          └──────────────┘
       │ capture continuity blob
       ▼
┌──────────────┐ profile 兼容             ┌──────────────┐
│ OpaqueReady  │─────────────────────────▶│  KeepOpaque  │
└──┬────────┬──┘                          └──────────────┘
   │        │
   │        │ mismatch + 有 fallback_text
   │        ▼
   │   ┌──────────────┐
   │   │  Downgraded  │
   │   └──────────────┘
   │ mismatch + 无 fallback_text
   ▼
┌──────────────┐
│ StripToText  │
└──────────────┘
```


| 当前状态          | 事件                               | 目标状态          | 副作用                                     | 说人话            |
| ------------- | -------------------------------- | ------------- | --------------------------------------- | -------------- |
| `NewTurn`     | 捕获到可见正文但无 continuity blob        | `VisibleOnly` | 只持久化 `assistant_text` / `thinking_text` | 普通回答，没有私有续传材料。 |
| `NewTurn`     | 捕获到 provider continuity blob     | `OpaqueReady` | 写入 `reasoning_continuation`             | 这轮拿到了可回放黑盒。    |
| `OpaqueReady` | 目标 profile 兼容                    | `KeepOpaque`  | 原样 replay opaque blob                   | 同类模型就高保真续。     |
| `OpaqueReady` | 目标 profile 不兼容且有 `fallback_text` | `Downgraded`  | 把 continuity 转为文本                       | 黑盒吃不下就换文字版。    |
| `OpaqueReady` | 目标 profile 不兼容且无 `fallback_text` | `StripToText` | 丢掉 blob，只保留可见历史                         | 最差也别把请求打挂。     |


---

## 9. 配置与环境变量

建议最小配置面如下；优先级统一为 **env > config > 默认**。


| 变量                                                                                                            | 取值           | 含义                                                       | 优先级               | 说人话                                                  |
| ------------------------------------------------------------------------------------------------------------- | ------------ | -------------------------------------------------------- | ----------------- | ---------------------------------------------------- |
| `TOMCAT__LLM__REASONING_CONTINUITY__ENABLED` / `[llm.reasoning_continuity] enabled`                           | `true/false` | 是否开启 continuity capture + replay 主能力；默认 `false`，以保持现网行为。 | env > config > 默认 | 先用总开关把新能力关住。                                         |
| `TOMCAT__LLM__OPENAI_RESPONSES__USE_PREVIOUS_RESPONSE_ID` / `[llm.openai_responses] use_previous_response_id` | `true/false` | 是否启用 OpenAI `previous_response_id` 优化；默认 `false`。**开启该开关意味着同时把请求切回 `store=true`，与默认 `store=false` 主线互斥。**        | env > config > 默认 | OpenAI 快车道单独开；开了就别再让 client 自己 replay 历史。                                       |
| `[llm.thinking] persist`（已存在）                                                                                 | `true/false` | 是否把 thinking 文本独立落盘。                                     | config            | 这只影响 thinking 文本审计，**不**应该决定 opaque continuity 是否存在。 |


说人话：用户只该碰两个开关: “要不要开启 continuity” 与 “OpenAI 要不要再开快车道”。`keep / strip / convert / downgrade` 这类细节不暴露给 TOML。

---

## 10. 错误模型 / 截断 / 警告

```text
正常 same-profile replay      → 正常请求
profile 不兼容               → warning_once + downgrade
DeepSeek 必需 reasoning 缺失 → Err（非可重试，避免继续 400）
OpenAI previous_response_id 失效 → warning + 退回路径 A（store=false + 显式 replay）重试一次
OpenAI opaque blob replay 失效  → warning + strip opaque / 降级为可见历史
think scrubber 发现半截标签    → 不向用户显示；流尾 flush 决定是否补回普通文本
```


| 风险点                                            | 归一化处理                                       | 说人话             |
| ---------------------------------------------- | ------------------------------------------- | --------------- |
| DeepSeek tool-call turn 丢失 `reasoning_content` | 显式返回协议错误，禁止静默继续发送已知错误请求。                    | 已知会 400 的请求别硬发。 |
| `deepseek-reasoner` 错带旧 `reasoning_content`    | 由 profile 提前 strip；若仍带出则视为实现 bug。           | 该删就删。           |
| OpenAI `previous_response_id` 过期 / 不匹配         | 视为优化失效，自动退回主线（`store=false` + 显式 replay）重试一次。 | 快车道堵了，就退回普通车道。  |
| OpenAI reasoning blob 无法 replay                | warning + downgrade 到可见历史或 `fallback_text`。 | 黑盒坏了也别把会话一并带崩。  |
| 未知 provider / model family                     | 默认 `strip opaque`，保留可见历史并 warn 一次。          | 不认识的模型先保守行事。    |


---

## 11. 测试矩阵（验收）


| 维度      | 用例 / 编号                                                             | 状态           | 说人话                                                |
| ------- | ------------------------------------------------------------------- | ------------ | -------------------------------------------------- |
| 单元      | `replay_policy_openai_responses_keeps_encrypted_reasoning`          | ✅ 2026-06-01 | OpenAI 同 profile 时要保留 opaque reasoning。            |
| 单元      | `replay_policy_deepseek_tool_turn_requires_reasoning_content`       | ✅ 2026-06-01 | DeepSeek 调过工具的那轮必须回放 reasoning。                    |
| 单元      | `replay_policy_deepseek_non_tool_turn_strips_reasoning_content`     | ✅ 2026-06-01 | DeepSeek 普通多轮不能乱带旧 reasoning。                      |
| 单元      | `cross_provider_downgrade_prefers_fallback_text`                    | ✅ 2026-06-01 | 跨 provider 时优先退到安全文本。                              |
| 单元      | `transcript_roundtrip_preserves_reasoning_continuation`             | ✅ 2026-06-01 | JSONL 来回一趟不能把 continuity 字段丢掉。                     |
| 集成      | `openai_responses_roundtrip_replays_reasoning_items`                | ✅ 2026-06-01 | OpenAI Responses 真正能 capture -> persist -> replay。 |
| 集成      | `deepseek_chat_roundtrip_replays_tool_turn_reasoning_content`       | BLOCKED（当前模型首轮只返回 `tool_calls`，未返回 `reasoning_content`） | DeepSeek（经 `provider=openai` + DeepSeek `api_base`）真接口已可访问，但当前实际返回还不足以形成 continuity snapshot。 |
| 集成      | `legacy_transcript_without_continuity_fields_still_hydrates`        | ✅ 2026-06-01 | 老 transcript 继续能聊。                                 |
| 单元 / 集成 | `streaming_think_scrubber_hides_split_tags`                         | ✅ 2026-06-01 | `<think>` 被拆成多段时也不能漏到用户。                           |
| 观察指标    | G1–G6 映射到上面 8 条用例 + docs review                                     | PARTIAL（差 DeepSeek 真 key） | 绝大多数锁点已落地；剩余 blocker 是 DeepSeek 真接口验收。              |
| 文档      | 本文 + `llm-multiprovider-integration.md` + `host-core-layer.md` 回链同步 | ✅ 2026-05-31 | 方案边界写清楚。                                           |


---

## 12. 风险与应对


| 风险                                     | 影响                                       | 应对（具体动作）                                                                             | 说人话                       |
| -------------------------------------- | ---------------------------------------- | ------------------------------------------------------------------------------------ | ------------------------- |
| 把 provider 私有字段直接写成 transcript 主结构     | transcript 被某家 wire 绑死，换 provider 时大面积返工 | continuity 一律包进 `reasoning_continuation{source_*, format, opaque_payload}`，禁止裸字段散落各处 | 黑盒可以存，但别把账本写成某家私有格式。      |
| 把 `previous_response_id` 当主 continuity | 离开 OpenAI 就断链                            | 明确把它降级为 provider_refs 优化字段，主真理仍是 transcript                                          | 快车道不能当主路。                 |
| 把 DeepSeek 规则继续写成全局 strip              | tool-call turn 继续 400                    | 引入 `had_tool_call + replay_requirement + model family profile`                       | DeepSeek 的坑已经不是一个布尔值能兜住的。 |
| continuity 提高后 CoT 泄漏到用户界面或日志          | 隐私 / 合规风险                                | `thinking_text` 与 `reasoning_continuation` 分层；补 think scrubber；默认不把 opaque blob 打日志  | 能续推理不代表能给人看。              |
| replay 材料过多导致 token / 成本上涨             | 长对话越聊越贵                                  | profile 驱动的 selective replay + no-tool strip + visible-history fallback              | 不是所有 thinking 都值得一直带着走。   |


---

## 13. 历史决策 / 跨文档修订

### 13.1 与相邻文档的边界


| 文档                                                                       | 负责什么                                                      | 不负责什么                                                                | 说人话                                  |
| ------------------------------------------------------------------------ | --------------------------------------------------------- | -------------------------------------------------------------------- | ------------------------------------ |
| `[llm-multiprovider-integration.md](llm-multiprovider-integration.md)`   | provider 抽象、Completions / Responses 主骨架、接新 provider 的总体接线 | cross-turn reasoning continuity 的 transcript / replay / downgrade 细则 | 它讲“怎么接模型”，本文讲“上一轮 thinking 怎么续到下一轮”。 |
| `[llm-stream-events-cli-pipeline.md](llm-stream-events-cli-pipeline.md)` | thinking 请求参数、stream event、CLI/TUI 展示                     | transcript 中 continuity 字段、跨 provider replay 决策                      | 它讲“thinking 怎么显示”，本文讲“thinking 怎么续”。 |
| `[host-core-layer.md](host-core-layer.md)`                               | 宿主核心层的总览入口                                                | reasoning continuity 的详细协议与状态机                                       | 它是索引入口，不展开这条子方案。                     |


### 13.2 本文对旧结论的修正

1. **修正旧粗规则**：`DeepSeek = resend 时 strip thinking` 已不足以描述官方现状；必须改为 tool-call-sensitive replay。
2. **明确 OpenAI 定位**：`previous_response_id` 是优化，且要求 `store=true`，与 Tomcat 默认 `store=false` 主线互斥，不是 shared transcript 的替代品。
3. **补 Hermes 边界**：Hermes 值得借鉴的是 replay / strip / scrubber 细节，不是把整套 transport 胶水层原样搬入 Tomcat。

### 13.3 修订记录


| 日期         | 说明                                                                                                           |
| ---------- | ------------------------------------------------------------------------------------------------------------ |
| 2026-05-31 | 初稿：冻结 OpenAI / DeepSeek 跨 turn reasoning continuity 主路线；新增术语、竞品证据、Hermes 专节、协议、One-Glance、时序、状态机、测试矩阵与跨文档边界。 |


