# Stream Terminal Errors

> 目标：把“provider 在流里送回来的终局错误为什么会直接判死一整轮、这次为什么不是 PDF 本身不合法、以及为什么我们的修法必须比别人多一层自愈”一次讲清楚。

## 内部通知的正常空终态

```text
本轮收下一条可见、正常 stop、无工具且无错误的答复 → 开启静默许可
  │
  ├─ 真人插话 / 非 Signal 追加输入 / 新运行入口 → 关闭许可，保留原空答保护
  └─ 后台 Signal / 完成守卫 Nudge / 工具调用 / 压缩 → 许可不变
       │
       └─ 正常 stop、无正文、无工具或畸形调用、无截断和错误
            ├─ 上一条不是静默，且还有额度 → 原样重发一次，排除偶发故障
            └─ 上一条也是静默，或已到最后额度 → 收下，走原 finalize
                 ├─ 计划还有待办 → 原守卫催促；连续两次无进展则交还计划
                 └─ 无待办 → 正常结束，不产生空回复红色错误

主动 Stopped → UI 保留终态，不再进入模型 follow-up（原行为不变）
```

`AgentLoop::silent_reply_allowed` 表示本轮已经收下一条可见答复、之后没有真人新输入。许可在 `reasoning_loop` 收下有正文、正常 `stop/end_turn`、无工具和错误的回复时打开，即使随后完成守卫继续催促也保留；`run_inner` 入口关闭，`inject_steering_messages` 实际注入消息后关闭，`inject_follow_up_messages` 取出非 Signal 消息时关闭。工具结果、Nudge、纯 Signal、被收下的静默和压缩不再撤销许可，删除原先 `run_inner` 的「整批纯 Signal 才授权」逻辑。

`last_reply_silent` 记录上一条模型回复是否为正常停止、无正文、无工具或畸形调用、未截断且无错误；每次解析完回复后更新，包含被拒绝重发的回复，传输失败没有新回复则不刷新。收下条件为 `silent_reply && silent_reply_allowed && (last_reply_silent || attempt >= max_attempts)`：第一条静默先确认一次，连续第二条才收下；额度只剩最后一次时直接收下（覆盖 `max_attempts=1` 和之前网络错误耗尽额度）。不能用 `attempt > 1` 代替连续静默，因为 attempt 是整段推理循环共用的预算。运行入口无需清零 `last_reply_silent`：许可已关闭，能重新开启许可的可见答复会先将它刷新为 false。

本规则只调整已有 hidden/thinking-only 空答检查的豁免条件，不重写其它纯空或终局处理。被收下的静默仍走原 finalize，保留 usage、reasoning continuation、finish reason、持久化、completion guard 和 TurnEnd；首次失败重发不插入催促，不改变输入消息。完成守卫、重试预算和等待间隔、催促文案及界面代码不改。

不从历史最后一条 assistant 或 `start_idx` 推断完成责任：压缩可将答复或未完成真人任务折入摘要；Retry/Resume 在 run 入口追加通知前已经复制/恢复真人工作，尾部仅见 Signal 不能证明无需回答。两个状态位只在可信的回复接收或输入注入点更新，不持久化、不倒推历史。CLI 空闲 auto_drain 入口本期仍不授权；普通真人新输入的 reasoning-only 保护不放宽。

### 事故与统计证据

```text
本次：最终答复 → 守卫催促 → update_plan → 连续静默
旧实现：许可已撤销 → 相同请求重发 9 次 → 约 8 分钟后报红错
新实现：首次静默确认一次 → 收下持续静默 → 原守卫继续催或交还计划
```

原整改计划的调查快照覆盖 279 个会话、16 次 hidden-output 空回合（这里按故障回合计数，不把每个重试请求当成一次）：

| 情形 | 回合数 | 每回合连续空回复 | 后续结果 |
| --- | ---: | --- | --- |
| 干活中途偶发静默 | 10 | 1 次 | 原样重发一次即恢复：7 次继续工具、3 次正文 |
| 答复后收到后台通知，再静默 | 4 | 4 次 | 旧实现重发到上限；10-05 的 `b7895282` 已增加 Signal 例外 |
| 答复后被催、调用工具，再静默 | 2 | 6 次、10 次 | 旧实现重发到上限仍失败 |

这 10 次偶发故障前，本轮也都已有过文字答复，不能用「给过答复」单独区分主动收尾和偶发故障。选择原样确认一次，是因为这份快照中的偶发故障都只空一次，而持续静默均连续四次以上；这只是当前样本证据，不保证以后永远不会连续偶发两次。统计来源：`~/.cursor/plans/交卷后静默收尾_2e84c13d.plan.md` 的「根因与证据」；本修复不新增真实模型请求。

本次会话 `1791639220943_02bdb744c9d5ba65` 的 transcript 第 485–505 行记录十次静默和九次重发。第 487 行是第二次请求的推理摘要（不是对用户的可见答复），其中明确写着 “keep the final response blank since the current task call will be complete”。其余九次没有可见推理摘要。该证据说明本次持续静默包含模型主动收尾的信号，不把隐藏推理的存在本身当作回答，也不把每次空答都归为上游故障。

09-12 的反例见 `1789145285030_4935347d04eeb062.jsonl`：第 1468、1927、1940、2006、2013、2021 行是六次偶发 hidden_output，均只需原样重发一次。原复盘在对应运行中计入六次守卫催促；若一见静默就直接收下，偶发故障也会占用守卫额度，合计十二次超过八次上限，08:37 那次偶发静默就可能提前让计划暂停。确认重发让恢复的偶发故障留在重试路径，不占守卫催促或空转计数。

### 参考实现与取舍

以下路径以同级工作区仓库为基准，已查证对应代码；不把参考实现引入 Tomcat 的依赖。

| 实现 | 查证位置 | 处理方式 |
| --- | --- | --- |
| Codex | `../codex/codex-rs/core/src/stream_events_utils.rs:270` 的 `finalize_non_tool_response_item`；`../codex/codex-rs/core/src/session/turn.rs:653` 的 `!needs_follow_up` 分支 | 空正文将最后答复置为 None；没有后续工作时进入正常收尾 |
| OpenCode | `../opencode/packages/opencode/src/session/prompt.ts:1081` 的 `runLoop`，尤其 1111–1129、1295–1307 | 正常停止且没有工具就退出；内容过滤另报错误，不用可见正文作为正常退出的必要条件 |
| Claude Code fork | `../cc-fork-01/src/services/api/claude.ts:2337-2350` 的 `stopReason` 守卫 | 已有 end_turn 的空正文是合法空响应，不等同于流未完成 |
| 反例 Cline | `../cline/sdk/packages/llms/src/providers/middleware/retry-empty-response.ts:97` 的 `DEFAULT_EMPTY_RESPONSE_MAX_ATTEMPTS` | 保留空响应重试中间件，默认共三次尝试（第一次加两次重试） |

Tomcat 没有照搬「空正文一律结束」：真人任务还没得到答复时保留现有保护；已经给过答复后用一次原样重发排除偶发故障，再把确认的静默交给既有完成守卫。代价是主动静默时多一个通常命中提示词缓存的请求，而不是九次重试后报错。重试过程只记入 transcript，扩展界面不展示；重试耗尽时由最终的错误卡片告知用户。

### 回归边界与推翻条件

`tomcat/src/core/agent_loop/tests/signal_completion_test.rs` 覆盖计划执行中的答复→工具→静默、一次相同输入确认、偶发恢复、最后额度、元数据保留、工具中途真人插话、静默时插话、新运行及压缩。事故用例先在旧实现上重试耗尽，再验证修复后 Completed、五个请求、各一条 `empty_turn`/`auto_retry_start`、`plan.stalled(reason=idle_nudges)` 和计划停放为 pending。它只用读取工具、不制造计划进度；真实 `update_plan` 改动会由原守卫认定为进展，可能再多一次催促，不改变首次静默只确认一次的规则。

新提问、Signal 入场但尚未答复、真人插话后的空答，以及截断、未知终态、畸形工具调用和推理泄漏，仍按已有保护处理。压缩后继续使用接收时记录的许可；不扫描历史猜测。`completion_guard_test.rs`、`run_basic_test.rs` 及 serve `commands_test.rs` 的既有守卫、预算和 hidden-output 边界继续回归。

出现以下证据时再推翻本次选择，不预先增加计数器、通知子类型协议或守卫新分支：

1. 偶发静默连续两次，被误收下并导致中途交还计划：将确认重发次数从一次提高到两次。
2. 后台通知实际需要用户等待结果汇报，却被静默收尾：按通知子类型收窄许可。
3. 完成守卫面对已确认静默仍反复空催成为常态：考虑确认静默后直接交还计划。

## 1. 说人话版

这次表面现象很吓人：

```text
你贴了一份 PDF
    ↓
界面马上弹红字
    ↓
同一句错误还重复显示了两遍
```

但真正的问题不是“PDF 发不出去”，而是：

```text
某个上游/中转节点在流里拒绝了这一次请求
    ↓
我们的流处理层把它当成“普通事件”吞在本轮里
    ↓
Attempt Loop 根本没接到这个错误
    ↓
本来该有的 4 次重试一次都没发生
    ↓
前端又把 reason 和 error_message 拼重了
```

这份修复做的不是“让 PDF 合法”，因为它本来就是合法的；做的是把这类**流内终局错误**重新送回重试主路，并在“连续两次都被说不支持多模态”时自动降级成纯文本重发。

## 2. 这次为什么不能怪 PDF

同一个 transcript 里，三条记录已经足够说明问题：

```text
时间       发出去的东西                                                结果
────────────────────────────────────────────────────────────────────────────────
02:43     文字 + 5 张图 + Algorithms Bayes.pdf                        成功
10:58     文字 + 完全同一份 Algorithms Bayes.pdf                      失败
10:59     文字（不带新附件），但历史里仍然带着前面的 input_file          成功
```

关键信息是“同一份 PDF、同一类 `input_file`、同一个 `fcodex/gpt-5.6-sol` 路由，前一次成功，中间一次失败，后一次又成功”。这排除了“模型永远不支持 PDF”这条解释，剩下的只有两种可能：

1. 中转/上游某个节点在 10:58 那一刻偶发拒绝。
2. 中转把请求形状重写成了另一种它自己不接受的样子。

无论是哪种，根因都不在“这份 PDF 不能发”。

## 3. 11 次真请求重放说明了什么

这次没有停在“像是偶发”这个层面，而是把 transcript 里的 10:58 那轮请求按 `payload.rs` 规则原样重建，直接打真实端点。

重放方式：

```text
transcript                  -> 取到当轮之前的全部消息
payload.rs 规则             -> 重新拼 instructions / input / include / store
真实 key + 真实 base_url     -> POST /v1/responses
终端只打印 shape 和终局事件   -> 不打印 base64
```

结果总结：

```text
实验                                                   结果
────────────────────────────────────────────────────────────────────
只发 10:58 那条（文字 + PDF）                           成功
发 02:43 成功轮 + 10:58 失败轮的完整历史                 成功
同级复杂度的真实 shape 连续重放多次                      全部成功
────────────────────────────────────────────────────────────────────
合计 11 次真实请求，0 次失败
```

仓库里新增的 `tomcat/scripts/replay-turn-payload.py` 就是把这次手工重放固化下来。它读取 transcript、选一条 user turn、按当前 `payload.rs` 的高层规则重建 `/v1/responses` 请求，并且只打印：

- 请求形状
- `previous_response_id` / `include` / `store` 等关键字段
- SSE 终局事件

不会把图片/PDF 的 base64 打到终端里。

## 4. 中转不是“纯透传”

这次实测还顺带坐实了一件更重要的事：`fcodex.top` 不是简单把我们的 JSON 转发给 OpenAI。

实测到的改写包括：

```text
我们发的                          对方实际回显/返回的行为
────────────────────────────────────────────────────────────────
"stream": false                   仍返回 SSE
reasoning.summary = "auto"        回显成了另一种值
没发 top_p                        中转自己补了 top_p
没发 temperature                  中转自己补了 temperature
上游真实坏请求                     统一被包装成 HTTP 502 + 通用错误文案
```

所以“中转不可能有问题，因为它只是透传”这个前提这次已经被实测推翻了。既然用户可以自配 `fcodex`、`litellm-sunmi` 这类中转，我们就不能把“能力表永远可信、错误形状永远稳定”当前提。

## 5. 两条错误回家路

修复前，系统里其实有两条错误回家路，其中一条能回到重试主路，另一条直接断在半路。

### 5.1 修复前

```text
                HTTP 4xx / 5xx
                      │
                      ▼
                classify_error
                      │
                      ▼
                 Attempt Loop
                      │
               Retry / Fatal


      stream event: LlmError / response.failed
                      │
                      ▼
               stream_handler emit_event
                      │
                      ▼
                 本轮直接收口
                 不进重试主路
```

这就是为什么明明 `agent_max_attempts = 4`，但这次 10:58 一次都没重试。

### 5.2 修复后

```text
      stream event: LlmError / response.failed
                      │
                      ▼
      stream_handler 先判断“有没有可用产出”
              │                      │
              │有文本/有 tool_call    │完全空回答
              ▼                      ▼
      保持原事件语义            合成 AppError::LlmDetailed
                                     │
                                     ▼
                               classify_error
                                     │
                ┌────────────────────┴────────────────────┐
                ▼                                         ▼
          确定性拒绝                               其余流内终局错误
      （如 content_filter）                         （可重试）
                │                                         │
                ▼                                         ▼
             Fatal                                  Attempt Loop
                                                         │
                                   原样重试一次 -> 连续再拒绝则降级重试
```

这次修复真正补上的，就是右边这条“流内终局错误也能回 Attempt Loop”。

## 6. 为什么我们的修法和别家不一样

### 6.1 先查过的五家实现

按 `.cursor/rules/prior-art-before-architecture.mdc` 的要求，这种跨进程错误契约变更必须先查证。实际查到的 5 家实现如下：

| 实现 | 做法 | 证据 |
| --- | --- | --- |
| OpenCode | 发送前把模型不支持的 part 直接替换成错误提示文本 | `opencode/packages/opencode/src/provider/transform.ts` 的 `unsupportedParts()` |
| VS Code Copilot | 发送前按能力表决定是否允许 PDF；不支持就只保留 reference | `vscode/extensions/copilot/src/platform/endpoint/common/chatModelCapabilities.ts` 的 `modelSupportsPDFDocuments()`；`vscode/extensions/copilot/src/extension/prompts/node/panel/fileVariable.tsx` 的 `FileVariable` |
| Codex | 模型不支持图片时，阻止提交并把内容还回输入框 | `codex/codex-rs/tui/src/chatwidget/input_submission.rs` 的 `restore_blocked_image_submission()` |
| Cline | 在出站前截断不支持的 document data | `cline/sdk/packages/core/src/session/services/message-builder.test.ts` 的 `truncates unsupported document data blocks` |
| Continue | 聊天附件白名单里根本没有 PDF | `continue/gui/src/components/mainInput/InputToolbar.tsx` 的 `accept=".jpg,.jpeg,.png,.gif,.svg,.webp"` |

### 6.2 从调研里得到的两个结论

正结论：

- 5/5 都在**发送前**处理不支持的多模态内容。
- 所以我们保留 `degrade_unsupported_multimodal()` 这条“发送前降级”的主路径是对的。

反结论：

- 5/5 都没有“流里来了终局错误，也要再回重试主路”这层机制。
- 原因不是他们都证明“不需要”，而是它们普遍没有把“用户自配第三方中转”当一等公民。

这正是这次最关键的反例：

```text
没有先例
  ≠ 这件事不该做

没有先例
  = 别家的边界条件比我们窄：
    自家端点 / 官方端点 / 能力表可信 / 错误形状相对稳定
```

Tomcat 的边界更宽，用户会直接把第三方 relay 塞进 `models.toml`。这时“上游偶发拒绝后要能自愈”就必须我们自己兜，不可能照抄别家的更窄假设。

## 7. 这次落地的策略

最终策略不是“看见 `input_file` 就硬删”，而是三段式：

```text
第一次流内终局拒绝
    -> 原样重试一次

如果连续第二次还是“当前端点不接受多模态”
    -> 在内存里把 input_image / input_file 降级成占位文本
    -> 发 llm_notice 告诉用户“已按纯文本发送”
    -> 再重试一次

如果是 content_filter 这类确定性拒绝
    -> 直接终局，不浪费 4 次预算
```

这里有两个刻意保持的边界：

1. **降级只改内存里的 messages，不改 transcript。** transcript 仍然保留最原始的用户输入账本。
2. **规则按“相对顺序”算，不按“第 3 次”写死。** 否则 `agent_max_attempts` 一变，行为就漂。

## 8. 什么情况下应该推翻今天这个决定

这部分必须写下来，避免以后把“当前修法”误当成永久真理。

### 8.1 如果重试开始掩盖我们自己的请求体 bug

如果以后出现“其实是我们字段拼错了，但因为一律重试导致白等 4 次”的案例，就该把“流内终局错误可重试”从大范围规则收窄成白名单，而不是继续加更多特例。

### 8.2 如果某个端点始终拒绝 `input_file`

今天的策略只能保证“不报错，按纯文本继续发”，但保证不了“模型一定还能读懂 PDF”。如果出现稳定拒绝 `input_file` 的端点，正确补法应该是：

```text
扩展宿主把 PDF 抽成文本
    ↓
再把抽出的文本发给模型
```

而不是在 Rust 里再引一棵 PDF 解析依赖树。这个边界跟 `read` 工具当年“不引图片/PDF 解码依赖、Cargo.lock 零增长”的决定一致，推翻它需要单独立项。

### 8.3 如果重试梯子又被写死成固定次数

降级触发条件必须永远是：

```text
先给一次原样重试机会
如果还失败，并且后面还剩至少一次尝试
再给一次降级重试
```

不能把它重新写回“第 3 次才降级”这种固定序号规则。

## 9. 相关代码入口

这次修复涉及的主链路可以从这里开始看：

- `tomcat/src/core/agent_loop/stream_handler.rs`
- `tomcat/src/core/agent_loop/error_classifier.rs`
- `tomcat/src/core/agent_loop/run.rs`
- `tomcat/src/api/chat/run_loop/mod.rs`
- `tomcat/src/core/llm/openai_responses/payload.rs`
- `tomcat/scripts/replay-turn-payload.py`

推荐阅读顺序：

```text
stream_handler.rs
  -> 为什么流内空回答错误现在会回 Attempt Loop

error_classifier.rs
  -> 为什么 content_filter 仍然 Fatal，unsupported multimodal 会 Retryable

run.rs
  -> 为什么是“先原样，再降级”，而且不依赖固定 attempt 序号

payload.rs
  -> transcript 到 /v1/responses body 的真实翻译规则

replay-turn-payload.py
  -> 怎么把某一轮 transcript 一键重放到真端点
```

## 10. 当前结论

这次事件的最终结论可以压成一句话：

> **请求体是合法的；真正的缺陷是我们把“流里送回来的终局错误”遗漏在了重试主路之外。**

所以修复点应该放在 Attempt Loop 和 stream terminal error 归并上，而不是继续怀疑“PDF 本身能不能发”。
