# Stream Terminal Errors

> 目标：把“provider 在流里送回来的终局错误为什么会直接判死一整轮、这次为什么不是 PDF 本身不合法、以及为什么我们的修法必须比别人多一层自愈”一次讲清楚。

## 内部通知的正常空终态

```text
主动 Stopped → UI 保留终态，不再进入模型 follow-up
正常收尾 + 实际取出的整批均为 Signal → 只许可下一次接受的响应静默
新提问 / Retry / Resume / 工具轮中途的通知 → 默认空答保护
```

`AgentLoop::silent_reply_allowed` 仅在 `run_inner` 正常完成、completion guard 放行且未耗尽工具轮预算后，从实际注入的新批次产生。入口清零；自动重试同一批输入时保留；任何响应一被接受就清零，后续工具结果与 Nudge 不继承许可。只有正常 `stop/end_turn`、无正文、无工具或畸形调用、未截断且无终局错误时可豁免 hidden/thinking-only 判定；仍走原 finalize 保留用量、continuity、持久化、completion guard 和 TurnEnd。

不从历史最后一条 assistant 或 `start_idx` 推断完成责任：压缩可将未完成真人任务折入摘要；Retry/Resume 在 run 入口追加通知前已经复制/恢复真人工作，尾部仅见 Signal 不能证明无需回答。CLI 空闲 auto_drain 入口本期不授权，静默仍可能重试报错；出现该类反馈时才在可信注入点扩展授权，不增加持久回执或改回历史猜测。若未来 Signal 要求行动，需结构化子类型限定许可；普通用户 reasoning-only 的保护不随本修复放宽。

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
