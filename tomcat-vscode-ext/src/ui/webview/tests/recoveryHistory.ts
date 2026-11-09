import type { SessionHistoryPayload } from "../../../serveClient/sessionRouter";

/** Synthetic incident shape: 79 completed calls, four failed attempts, then an
 * error. The 80-row page starts on a result, so its owning assistant makes 81.
 * No original user content, transcript IDs, paths, or credentials are retained. */
export function longFailedToolHistory(sessionId = "s1") {
  const messages: unknown[] = [
    { type: "message", id: "long-user", message: { role: "user", content: "Inspect the fixture" } },
  ];
  for (let index = 1; index <= 79; index += 1) {
    messages.push(
      { type: "message", id: `long-assistant-${index}`, message: {
        role: "assistant", content: null,
        tool_calls: [{ id: `long-call-${index}`, function: { name: "read", arguments: '{"path":"fixture.txt"}' } }],
      } },
      { type: "message", id: `long-result-${index}`, message: {
        role: "tool", tool_call_id: `long-call-${index}`, content: "Completed fixture result",
      } },
    );
  }
  for (let attempt = 1; attempt <= 4; attempt += 1) {
    messages.push({ type: "custom", id: `long-failure-${attempt}`, event: "llm_error", code: "server_error" });
    if (attempt < 4) messages.push({ type: "custom", id: `long-retry-${attempt}`, event: "auto_retry_start", attempt: attempt + 1, max_attempts: 4 });
  }
  messages.push(
    { type: "custom", id: "long-retry-end", event: "auto_retry_end", success: false, attempt: 4 },
    { type: "error", id: "long-error", summary: "Upstream overloaded", detail: "Synthetic server_error" },
  );
  const full: SessionHistoryPayload = { sessionId, messages, hasMore: false };
  const tail: SessionHistoryPayload = {
    sessionId, messages: messages.slice(-81), hasMore: true, nextCursor: "long-assistant-44",
  };
  return { full, tail };
}
