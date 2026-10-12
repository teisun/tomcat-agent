import { describe, expect, it } from "vitest";
import { setLocale, translate } from "./i18n";

import {
  ConnectorProtocolError,
  normalizeConnectorView,
  parseConnectorProject,
  parseProjectTrustPayload,
  parseConnectorReloadReceipt,
  parseConnectorToolCatalog,
  parseSetConnectorToolEnabledResponse,
} from "./connectorsProtocol";

describe("connector recovery contract", () => {
  it("keeps the protocol error identity when its message is Chinese", () => {
    expect(() => parseConnectorReloadReceipt({}, "key")).toThrow(translate("en", "connector.protocolMismatch"));
    setLocale("zh-CN");
    try {
      expect(() => parseConnectorReloadReceipt({}, "key")).toThrow(ConnectorProtocolError);
    } finally { setLocale("en"); }
  });
  const receipt = { configKey: "key", accepted: true, generation: "18446744073709551615", recoveryTimeoutMs: 111_250 };
  it("keeps u64 generations as exact text and separates acceptance from success", () => {
    expect(parseConnectorReloadReceipt(receipt, "key")).toEqual(receipt);
    expect(() => parseConnectorReloadReceipt({ ...receipt, reloaded: true, accepted: undefined }, "key")).toThrow(ConnectorProtocolError);
  });
  it.each([
    { reloaded: true },
    { ...receipt, configKey: "another" },
    { ...receipt, accepted: false },
    ...[0, "0", "01", "-1", "1e3", "18446744073709551616"].map((generation) => ({ ...receipt, generation })),
    ...[0, -1, 1.5, Infinity, Number.MAX_SAFE_INTEGER].map((recoveryTimeoutMs) => ({ ...receipt, recoveryTimeoutMs })),
  ])("rejects an old or invalid receipt without inventing its identity: %j", (value) => {
    expect(() => parseConnectorReloadReceipt(value, "key")).toThrow(ConnectorProtocolError);
  });
  it("requires directory identity from the same backend snapshot", () => {
    expect(parseConnectorToolCatalog({ configKey: "key", generation: "7", attempt: 2, tools: [] }, "key")).toMatchObject({ generation: "7", attempt: 2 });
    expect(() => parseConnectorToolCatalog({ configKey: "key", tools: [] }, "key")).toThrow(ConnectorProtocolError);
    expect(() => parseConnectorToolCatalog({ configKey: "key", generation: "7", attempt: 0, tools: [] }, "key")).toThrow(ConnectorProtocolError);
  });
  it("marks missing list identity as incompatible instead of silently using generation zero", () => {
    expect(normalizeConnectorView({ configKey: "key", name: "test", source: "global", state: "connected" })).toMatchObject({ compatibilityError: new ConnectorProtocolError().message });
    expect(normalizeConnectorView({ configKey: "key", name: "test", source: "global", state: "connecting", generation: "7", attempt: 2, recovery: { phase: "starting", maxAttempts: 3, remainingMs: 55_000 } })).toMatchObject({ generation: "7", attempt: 2, compatibilityError: undefined });
  });
});


describe("project trust contract", () => {
  it("reads project status without accepting an old per-service trust field", () => {
    expect(parseConnectorProject({ root: "/project", trusted: false })).toEqual({ root: "/project", trusted: false });
    expect(parseConnectorProject(null)).toBeNull();
    expect(() => parseConnectorProject({ trusted: true })).toThrow(ConnectorProtocolError);
    expect(parseProjectTrustPayload({ projectRoot: "/project", trusted: false })).toMatchObject({ projectRoot: "/project", trusted: false });
    expect(parseProjectTrustPayload({ projectRoot: "/project", trusted: false, error: "corrupt file" }).trusted).toBe(false);
    expect(() => parseProjectTrustPayload({ projectRoot: "/project", trusted: true, error: "corrupt file" })).toThrow(ConnectorProtocolError);
  });
  it("normalizes the pending state without per-connector approval", () => {
    const base = { configKey: "project", name: "browser", source: "workspace", generation: "0", attempt: 0, recovery: null };
    expect(normalizeConnectorView({ ...base, state: "awaiting_project_trust" })?.state).toBe("awaiting_project_trust");
    expect(normalizeConnectorView({ ...base, state: "needs_confirmation" })).toBeNull();
    expect(normalizeConnectorView({ ...base, state: "blocked" })).toBeNull();
    expect(normalizeConnectorView({ ...base, state: "connected", trust: "trusted" })).not.toHaveProperty("trust");
  });
});

describe("single-tool toggle response contract", () => {
  const expected = { configKey: "key", rawName: "capture", enabled: false };

  it.each([
    [true, true, true],
    [false, false, false],
    [false, true, false],
  ])("accepts the legal success/save/apply combination %j", (success, configSaved, runtimeApplied) => {
    expect(parseSetConnectorToolEnabledResponse({ ...expected, configSaved, runtimeApplied }, expected)).toEqual({ ...expected, configSaved, runtimeApplied });
    expect(success).toBeTypeOf("boolean");
  });

  it.each([
    {},
    { ...expected, configKey: "other", configSaved: true, runtimeApplied: true },
    { ...expected, rawName: "status", configSaved: true, runtimeApplied: true },
    { ...expected, enabled: true, configSaved: true, runtimeApplied: true },
    { ...expected, configSaved: "true", runtimeApplied: true },
    { ...expected, configSaved: true, runtimeApplied: null },
  ])("rejects an incomplete or mismatched payload: %j", (payload) => {
    expect(() => parseSetConnectorToolEnabledResponse(payload, expected)).toThrow(ConnectorProtocolError);
  });
});
describe("normalizeConnectorView", () => {
  it("accepts only the current canonical connector contract", () => {
    expect(normalizeConnectorView({
      configKey: "mcp:/home/me/.tomcat/mcp.json:github",
      configPath: "~/.tomcat/mcp.json",
      name: "github",
      overridden: true,
      source: "global",
      state: "connected",
    })).toMatchObject({
      configKey: "mcp:/home/me/.tomcat/mcp.json:github",
      configPath: "~/.tomcat/mcp.json",
      overridden: true,
      source: "global",
    });
  });

  it("rejects removed legacy source aliases and missing config identity", () => {
    expect(normalizeConnectorView({
      name: "playwright",
      source: "User",
      state: "connected",
    })).toBeNull();
    expect(normalizeConnectorView({
      name: "workspace-browser",
      source: "Workspace",
      state: "connected",
    })).toBeNull();
  });
});
