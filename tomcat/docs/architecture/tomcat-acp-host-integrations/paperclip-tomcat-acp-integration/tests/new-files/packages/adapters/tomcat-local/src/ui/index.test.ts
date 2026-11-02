import { describe, expect, it } from "vitest";
import { parseTomcatStdoutLine } from "./index.js";

describe("Tomcat transcript", () => {
  it("does not turn a receipt-free terminal frame into zero tokens or zero dollars", () => {
    expect(parseTomcatStdoutLine(JSON.stringify({ type: "acpx.result", stopReason: "end_turn" }), "time"))
      .toEqual([{ kind: "system", ts: "time", text: "end_turn" }]);
  });
  it("reuses the shared text and tool parser", () => {
    expect(parseTomcatStdoutLine(JSON.stringify({ type: "acpx.text_delta", text: "hello" }), "time"))
      .toEqual([{ kind: "assistant", ts: "time", text: "hello", delta: true }]);
    expect(parseTomcatStdoutLine(JSON.stringify({ type: "acpx.tool_call", toolCallId: "tool-1", name: "bash", status: "completed" }), "time"))
      .toMatchObject([{ kind: "tool_call", name: "bash", toolUseId: "tool-1" }, { kind: "tool_result", isError: false }]);
  });
});
