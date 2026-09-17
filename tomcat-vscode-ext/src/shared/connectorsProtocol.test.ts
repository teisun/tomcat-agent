import { describe, expect, it } from "vitest";

import { normalizeConnectorView } from "./connectorsProtocol";

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
