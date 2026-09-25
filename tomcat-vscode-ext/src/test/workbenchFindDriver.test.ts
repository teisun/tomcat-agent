import { describe, expect, it } from "vitest";
import { assertCleanSettingsConsole, settingsConsoleReport } from "./suite/support/workbenchFindDriver";

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
