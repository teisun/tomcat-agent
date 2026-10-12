import { describe, expect, it, vi } from "vitest";
import { assertCleanSettingsConsole, probeRenderedContext, settingsConsoleReport } from "./suite/support/workbenchFindDriver";

describe("read-only frame discovery", () => {
  it("skips a vanished context and probes a replacement without replaying the first request", async () => {
    const send = vi.fn()
      .mockRejectedValueOnce(new Error("CDP -32000: Cannot find context with specified id"))
      .mockResolvedValueOnce({ result: { value: { assets: ["https://owned/image-preview.js"] } } });
    expect(await probeRenderedContext({ send }, 1, '[data-testid="preview-stage"]')).toBeNull();
    expect(await probeRenderedContext({ send }, 2, '[data-testid="preview-stage"]')).toEqual({ assets: ["https://owned/image-preview.js"] });
    expect(send.mock.calls.map(([, params]) => params.contextId)).toEqual([1, 2]);
    expect(send.mock.calls.every(([method]) => method === "Runtime.evaluate")).toBe(true);
    expect(send.mock.calls[0][1].expression).not.toContain(".click(");
  });
  it("does not swallow or retry unrelated protocol failures", async () => {
    const error = new Error("CDP -32000: permission denied");
    const send = vi.fn().mockRejectedValue(error);
    await expect(probeRenderedContext({ send }, 1, ".tc-shell")).rejects.toBe(error);
    expect(send).toHaveBeenCalledOnce();
  });
});

describe("Settings-scoped browser console acceptance", () => {
  it("fails for a Settings error even when the chat error array is empty", () => {
    const chatErrors: unknown[] = [];
    const report = settingsConsoleReport([
      { method: "Runtime.consoleAPICalled", params: { executionContextId: 42, type: "error", args: [{ value: "SETTINGS-CHECKER-PROBE" }] } },
      { method: "Runtime.consoleAPICalled", params: { executionContextId: 7, type: "error", args: [{ value: "unrelated-chat-error" }] } },
    ], 42, ["https://owned-assets/gui/dist/settings.js"]);
    expect(chatErrors).toEqual([]);
    expect(report.errors).toHaveLength(1);
    expect(() => assertCleanSettingsConsole(report)).toThrow("SETTINGS-CHECKER-PROBE");
    expect(JSON.stringify(report)).not.toContain("unrelated-chat-error");
  });
  it("keeps warnings and fails for owned exceptions and asset errors only", () => {
    const report = settingsConsoleReport([
      { method: "Runtime.consoleAPICalled", params: { executionContextId: 42, type: "warning" } },
      { method: "Runtime.exceptionThrown", params: { exceptionDetails: { executionContextId: 42, text: "owned failure" } } },
      { method: "Runtime.exceptionThrown", params: { exceptionDetails: { executionContextId: 7, text: "other frame" } } },
      { method: "Log.entryAdded", params: { entry: { url: "https://owned-assets/gui/dist/chunks/settings-dependency.js", level: "error" } } },
      { method: "Log.entryAdded", params: { entry: { url: "https://another-window/gui/dist/settings.js", level: "error" } } },
    ], 42, ["https://owned-assets/gui/dist/settings.js"]);
    expect(report.events).toHaveLength(3);
    expect(report.errors).toHaveLength(2);
    expect(() => assertCleanSettingsConsole(report)).toThrow("owned failure");
    expect(() => assertCleanSettingsConsole(settingsConsoleReport([], 42, []))).not.toThrow();
  });
});
