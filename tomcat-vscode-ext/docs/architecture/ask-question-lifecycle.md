# Ask Question lifecycle

`ask_question` is a durable tool round with a live, host-owned answer channel. It has **no wall-clock timeout**. A normal answer, skip, or explicit interrupt produces a terminal result; a process restart produces a durable `[pending]` placeholder instead, so the same question can be continued after reconnection.

```text
assistant message (tool call, toolCallId)
                  |
                  v
Rust runtime -> serve control_request (sessionId, requestId)
                  |
                  v
Extension host pending map (sessionId + requestId + child generation)
                  |
                  v
Webview fixed “Needs your answer” panel
                  |
       answer / skip / explicit interrupt
                  v
serve control_response or terminal control_cancel
                  |
                  v
tool result message (same toolCallId) -> history AnswerCard

restart before a result
                  |
                  v
hydrate writes `[pending]` -> history ApprovalCard + fresh control_request
                  |
                  v
answer supersedes `[pending]`, appends the real result, starts a new turn
```

## Identities

| Identity | Lifetime | Purpose |
| --- | --- | --- |
| `toolCallId` | durable transcript | Joins the assistant tool declaration and tool-result companion. |
| `sessionId` + `requestId` | one live question | Routes answers and prevents a response from crossing sessions. |
| child generation | one `tomcat serve` process | Stops a deferred handler from writing to a replacement child after restart/exit. |

## Terminal outcomes

| Outcome | Meaning |
| --- | --- |
| `answered` | User submitted the request; individual questions may still carry `skipped: true`. |
| `skipped` | User skipped the entire request. |
| `interrupted` | The current turn was explicitly interrupted. |
| `host_disconnected` | A live transport positively confirmed that its host disconnected. It is retained for old transcripts; restart recovery treats the old synthetic form as resumable. |
| `cancelled_unknown` | Read-only compatibility label for an old `cancelled: true` payload without a typed outcome. |

A webview DOM reload is not terminal. The Provider reprojects an existing pending question into the new DOM. For a backend, frontend, or machine restart, hydrate first creates `[pending]`; the new connection then registers a new `requestId` for the same durable `toolCallId`. A stale answer for the old `requestId` remains rejected.

## UI state

- Unresolved questions render outside the scrolling transcript, immediately above the composer controls.
- The answer draft is keyed by `sessionId + requestId`, persisted with VS Code webview state, and guarded against duplicate submission.
- Background sessions expose a pending count badge. Switching to one focuses the first answer control and announces the question once through `aria-live`.
- History cards always show every question and option, including answered, question-level skip, whole-request skip, interrupt, disconnect, and legacy unknown states.

## Durable history

The assistant tool declaration and its tool result are the source of truth. `get_messages` expands a page that begins with a tool result to include its assistant companion, joined by `toolCallId`. Old custom ask-question entries are rendered only when the standard companion is missing.

This replaces the previous design, which repaired a dangling `ask_question` with a terminal `host_disconnected` result. Hydration now makes every tool round structurally valid immediately:

- a restart-dangling `ask_question` receives `[pending]`;
- other dangling tools receive an explicit “unknown after restart” terminal result;
- an explicit user interrupt remains `[interrupted]`.

`[pending]` is a synthetic, non-terminal result. When the question is answered or skipped, the old row is marked `superseded` and a new real tool result is appended under the same `toolCallId`. Consumers must ignore superseded messages; this preserves the historical timeline without presenting two tool results to an LLM.

When the user sends a new prompt instead of answering, the session append gate first changes every tail `[pending]` question into `skipped`, then appends that prompt. This rule lives in the session layer so CLI, serve, and plugin append paths cannot disagree.

## Decision boundaries

Revisit this design if any of these becomes true:

1. `ask_question` gains an external side effect; it would no longer be `replay_safe`.
2. A question can become stale while disconnected, for example when its options name mutable code locations.
3. `[pending]` appears frequently in provider requests; that signals a missing resolve path and the resolution must move into request construction.
4. Two windows can produce more than one active result for a `toolCallId`; recovery must then fall back to a first-answer-wins lock.

## Removed timeout configuration

These old settings are detected only to emit a migration warning and are otherwise ignored:

- `ask_question.timeout_ms`
- `TOMCAT_ASK_QUESTION_TIMEOUT_MS`
- `TOMCAT__ASK_QUESTION__TIMEOUT_MS`

They never create a deadline. There is no replacement timeout option.
