import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ConnectorsSettingsView } from "./ConnectorsSettingsView";
import type { SettingsStateSnapshot, VsCodeApiLike } from "../../../src/shared/settingsProtocol";

function state(): SettingsStateSnapshot {
  return {
    capabilities: {
      listModels: false,
      listProviderKeys: false,
      removeModel: false,
      setProviderKey: false,
      upsertModel: false,
    },
    connectorTools: [
      {
        description: "Click an element.",
        enabled: true,
        label: "browser_click",
        modelName: "mcp__playwright__browser_click",
        rawName: "browser_click",
      },
      {
        description: "Close the browser.",
        enabled: false,
        label: "browser_close",
        modelName: "mcp__playwright__browser_close",
        rawName: "browser_close",
      },
    ],
    connectors: [
      {
        command: "npx",
        configPath: "~/.tomcat/mcp.json",
        configPathRaw: "/tmp/.tomcat/mcp.json",
        name: "playwright",
        oauthConfigured: false,
        resourceCount: 0,
        source: "Global",
        state: "connected",
        toolCount: 2,
        transport: "stdio",
        trust: "trusted",
        type: "mcp",
      },
    ],
    models: [],
    providerKeys: [],
    ready: true,
    route: "connectors",
    selectedConnector: "playwright",
  };
}

function renderView(snapshot: SettingsStateSnapshot = state()) {
  const postMessage = vi.fn();
  const vscodeApi: VsCodeApiLike = {
    postMessage,
    setState: vi.fn(),
  };
  const rendered = render(<ConnectorsSettingsView state={snapshot} vscodeApi={vscodeApi} />);
  return { container: rendered.container, postMessage };
}

describe("ConnectorsSettingsView", () => {
  it("explains the saved configuration and connection without transport jargon", async () => {
    const { container, postMessage } = renderView();

    fireEvent.click(screen.getByRole("button", { name: /playwright/i }));

    await screen.findByText("Tools (2)");
    expect(screen.getByText("Config file")).toBeTruthy();
    expect(screen.getByText("~/.tomcat/mcp.json")).toBeTruthy();
    expect(screen.getByText("Connection")).toBeTruthy();
    expect(screen.queryByText("Transport")).toBeNull();
    expect(container.querySelector(".tc-connector-tool__indicator--enabled")).toBeTruthy();
    expect(container.querySelectorAll(".tc-connector-tool__indicator")).toHaveLength(2);
    fireEvent.click(screen.getByRole("button", { name: "~/.tomcat/mcp.json" }));
    expect(postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        data: { name: "playwright" },
        type: "openConnectorConfig",
      }),
    );
  });

  it("routes the Models navigation button to the models settings route", () => {
    const { postMessage } = renderView();

    fireEvent.click(screen.getByRole("button", { name: "Models" }));

    expect(postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        data: { route: "models" },
        type: "settings.ready",
      }),
    );
  });

  it("uses the backend-provided Global configuration file by default", () => {
    const snapshot = state();
    snapshot.connectorConfigPaths = {
      global: { display: "~/.tomcat/mcp.json", raw: "/tmp/home/.tomcat/mcp.json" },
      workspace: { display: ".workspace-data/mcp.json", raw: "/tmp/project/.workspace-data/mcp.json" },
    };
    const { postMessage } = renderView(snapshot);

    fireEvent.click(screen.getByRole("button", { name: /add connector/i }));

    expect(
      screen.getByRole("radio", { name: "Global" }),
    ).toHaveProperty("checked", true);
    fireEvent.click(screen.getByRole("button", { name: "~/.tomcat/mcp.json" }));
    expect(postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        data: { scope: "user" },
        type: "openConnectorConfig",
      }),
    );
  });

  it("uses the exact workspace configuration path for a connector added from an empty list", () => {
    const snapshot = state();
    snapshot.connectors = [];
    snapshot.connectorConfigPaths = {
      global: { display: "~/.tomcat/mcp.json", raw: "/tmp/home/.tomcat/mcp.json" },
      workspace: { display: ".workspace-data/mcp.json", raw: "/tmp/project/.workspace-data/mcp.json" },
    };
    const { postMessage } = renderView(snapshot);

    fireEvent.click(screen.getByRole("button", { name: /add connector/i }));
    fireEvent.click(screen.getByRole("radio", { name: "Workspace" }));
    fireEvent.click(screen.getByRole("button", { name: ".workspace-data/mcp.json" }));

    expect(postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        data: { scope: "workspace" },
        type: "openConnectorConfig",
      }),
    );
  });
});
