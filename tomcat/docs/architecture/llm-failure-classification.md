# LLM 失败归一化与恢复策略

## 问题

HTTP 状态码不能代表失败语义：

```text
403 + "forbidden"               -> 鉴权错误，自动重试无意义
403 + "insufficient balance"    -> 账户余额不足，等待用户充值后可手动重试
400 + "upstream request failed" -> 中转站暂态错误，应短退避重试
SSE 流内 context overflow       -> 没有 HTTP 状态，仍应触发上下文压缩
```

因此 `LlmFailureKind` 表示“发生了什么”，`FailureDomain` 表示“应该由哪类恢复策略处理”。两者写入 transcript 的 ErrorEntry，避免事故只能靠回读源码逆推。

## 判定顺序

判定入口是 `infra/error/llm.rs::classify_llm_failure`：

```text
已知 error.code
  -> error.type
    -> HTTP status
      -> summary 文本语料
```

账户类有一条额外纪律：Billing 的 code/type/text 证据必须先于 401/403/429 的通用状态判定。孤立的 403 仍然是 Authentication，孤立的 429 仍然是 RateLimit。

## 恢复策略

| Kind | Domain | 自动行为 | 人工行为 |
|---|---|---|---|
| `context_overflow` | Context | 只在 payload 真实变小时重试；第二次直接 Collapse | `/compact` 或 `/restore` |
| `billing` | Account | 不自动重试 | 充值或切换 Provider 后 Retry |
| `authentication` | Account | 不自动重试 | 修复凭据后 Retry |
| `rate_limit` / `upstream_transient` / `stream_interrupted` | Transport | 短退避重试 | Retry |
| `content_filtered` / `invalid_request` | Content / Request | 不自动重试 | 修正请求 |

“真实变小”是双重不变量：消息条数和估算字符数都必须下降。否则再次发送的是同一 payload，消耗 attempt 不会取得进展。

## 参考实现与取舍

| 实现 | 证据 | 采纳 / 不采纳 |
|---|---|---|
| Cline | `apps/vscode/src/services/error/ClineError.ts::getErrorType` 先判 `SPEND_LIMIT_EXCEEDED`，再判通用 429 | 采纳账户错误先于限流的顺序纪律 |
| Claude Code fork | `src/utils/messages.ts::SYNTHETIC_TOOL_RESULT_PLACEHOLDER` 明确标记假 tool result，供训练数据导出过滤 | 采纳“合成内容必须可识别、可清洗”的数据纪律 |
| Codex | `codex-rs/core/src/context/turn_aborted.rs::TurnAborted::INTERRUPTED_GUIDANCE` 提醒工具可能部分执行 | 采纳“结果未知时必须先核实”的语义，不把掉线伪装成用户取消 |

## 何时推翻

1. 如果 provider 能稳定给出结构化、跨供应商一致的错误分类，文本语料应降为最后的兼容层。
2. 如果观测到某种 `upstream_transient` 连续重试仍不改变任何输入/服务状态，应给它增加“重试必须取得进展”的专属判据，而不是加大次数。
3. 如果新的中转站把 Billing 与 Auth 编码为相同且无文本差异的状态，必须保守归为 Authentication，不能猜测为 Billing。
