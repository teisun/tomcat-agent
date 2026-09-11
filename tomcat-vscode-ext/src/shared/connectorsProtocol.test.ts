import { describe, expect, it } from "vitest";

import { normalizeConnectorView } from "./connectorsProtocol";

describe("normalizeConnectorView", () => {
  it("keeps legacy payload source metadata without guessing a file path", () => {
    const legacy = normalizeConnectorView({
      name: "playwright",
      source: "User",
      state: "connected",
    });
    const current = normalizeConnectorView({
      configPath: "~/.tomcat/mcp.json",
      name: "github",
      source: "Global",
      state: "connected",
    });

    expect(legacy).toMatchObject({
      configPath: null,
      source: "Global",
    });
    expect(current).toMatchObject({
      configPath: "~/.tomcat/mcp.json",
      source: "Global",
    });
  });

  it("keeps workspace configuration unavailable when a legacy payload omits it", () => {
    const legacyWorkspace = normalizeConnectorView({
      name: "workspace-browser",
      source: "Workspace",
      state: "connected",
    });

    expect(legacyWorkspace).toMatchObject({
      configPath: null,
      source: "Workspace",
    });
  });
});
