import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ConnectorsSettingsView } from "./ConnectorsSettingsView";
import { LocaleProvider } from "../i18n/LocaleProvider";
import { translate } from "../../../src/shared/i18n";
import type { SettingsStateSnapshot, VsCodeApiLike } from "../../../src/shared/settingsProtocol";

function state(): SettingsStateSnapshot {
  return {
    capabilities: {
      listModels: false,
      listProviderKeys: false,
      removeModel: false,
      setProviderKey: false,
      upsertModel: false,      connectorCapabilities: {
        add: true,
        filter: true,
        list: true,
        listTools: true,
        login: true,
        reload: true,
        remove: true,
        toggle: true,
        trustProject: true,
      },
    },
    connectorToolsIdentity: { configKey: "mcp:/tmp/.tomcat/mcp.json:playwright", generation: "1", attempt: 1 },
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
        configKey: "mcp:/tmp/.tomcat/mcp.json:playwright",
        configPath: "~/.tomcat/mcp.json",
        configPathRaw: "/tmp/.tomcat/mcp.json",
        name: "playwright",
        oauthConfigured: false,
        overridden: false,
        resourceCount: 0,
        source: "global",
        state: "connected",
        generation: "1",
        attempt: 1,
        recovery: null,
        toolCount: 2,
        transport: "stdio",
        type: "mcp",
      },
    ],
    models: [],
    providerKeys: [],
    ready: true,
    route: "connectors",
    selectedConnector: "mcp:/tmp/.tomcat/mcp.json:playwright",  };
}

function renderView(snapshot: SettingsStateSnapshot = state()) {
  const postMessage = vi.fn();
  const vscodeApi: VsCodeApiLike = {
    postMessage,
    setState: vi.fn(),
  };
  const rendered = render(<ConnectorsSettingsView state={snapshot} vscodeApi={vscodeApi} />);
  return { container: rendered.container, postMessage, rerender: (next: SettingsStateSnapshot) => rendered.rerender(<ConnectorsSettingsView state={next} vscodeApi={vscodeApi} />) };
}

const upstreamToggleError = "此工具被配置中的批量规则禁用，请打开配置文件修改。";

describe("ConnectorsSettingsView", () => {
  it("preserves form inputs, validation and card identity across language changes", () => {
    const snapshot = state();
    const api = { postMessage: vi.fn() };
    const view = render(<LocaleProvider locale="en"><ConnectorsSettingsView state={snapshot} vscodeApi={api} /></LocaleProvider>);
    const card = screen.getByTestId("connector-card-playwright");
    fireEvent.click(screen.getByTestId("connector-add-open"));
    fireEvent.click(screen.getByTestId("connector-add-submit"));
    const validation = screen.getByText(translate("en", "connector.nameRequired"));
    const command = screen.getByRole("textbox", { name: translate("en", "connector.command") });
    const name = screen.getByTestId("connector-name");
    fireEvent.change(name, { target: { value: "my-connector" } });
    view.rerender(<LocaleProvider locale="zh-CN"><ConnectorsSettingsView state={snapshot} vscodeApi={api} /></LocaleProvider>);
    expect(screen.getByTestId("connector-name")).toBe(name);
    expect(name).toHaveProperty("value", "my-connector");
    expect(screen.getByTestId("connector-card-playwright")).toBe(card);
    expect(validation.isConnected).toBe(true);
    expect(screen.getByTestId("connector-command")).toBe(command);
    fireEvent.click(screen.getByTestId("connector-transport-http"));
    expect(screen.getByRole("textbox", { name: "URL" })).toBeTruthy();
    expect(api.postMessage.mock.calls.some(([m]) => m.type === "addConnector")).toBe(false);
  });

  it("settles login by request identity, never translated status text", () => {
    const snapshot = state();
    const configKey = snapshot.connectors![0].configKey;
    Object.assign(snapshot.connectors![0], { transport: "http", oauthConfigured: true, state: "needs_authorization" });
    const api = { postMessage: vi.fn() };
    const view = render(<LocaleProvider locale="en"><ConnectorsSettingsView state={snapshot} vscodeApi={api} /></LocaleProvider>);
    fireEvent.click(screen.getByTestId("connector-card-playwright"));
    fireEvent.click(screen.getByTestId("connector-login"));
    const request = api.postMessage.mock.calls.find(([m]) => m.type === "loginConnector")![0];
    const authorizing = { ...snapshot, status: "正在授权连接器…", connectorLogin: { configKey, requestId: request.messageId, phase: "authorizing" as const } };
    view.rerender(<LocaleProvider locale="zh-CN"><ConnectorsSettingsView state={authorizing} vscodeApi={api} /></LocaleProvider>);
    expect(screen.getByTestId("connector-login")).toHaveProperty("disabled", true);
    expect(screen.getByTestId("connector-cancel-login")).toBeTruthy();
    view.rerender(<LocaleProvider locale="zh-CN"><ConnectorsSettingsView state={{ ...authorizing, status: "任意状态", connectorLogin: { configKey, requestId: "other", phase: "settled" } }} vscodeApi={api} /></LocaleProvider>);
    expect(screen.getByTestId("connector-login")).toHaveProperty("disabled", true);
    view.rerender(<LocaleProvider locale="zh-CN"><ConnectorsSettingsView state={{ ...authorizing, connectorLogin: { configKey, requestId: request.messageId, phase: "settled" } }} vscodeApi={api} /></LocaleProvider>);
    expect(screen.getByTestId("connector-login")).toHaveProperty("disabled", false);
    expect(api.postMessage.mock.calls.filter(([m]) => m.type === "loginConnector")).toHaveLength(1);
  });

  it("shows busy before Host replies, sends one Reload, and does not block another source", () => {
    const snapshot = state();
    snapshot.connectors!.push({ ...snapshot.connectors![0], name: "other", configKey: "other" });
    const view = renderView(snapshot);
    fireEvent.click(screen.getByRole("button", { name: /playwright/i }));
    const reload = screen.getByTestId("connector-reload");
    fireEvent.click(reload); fireEvent.click(reload);
    expect(reload).toHaveProperty("disabled", true);
    expect(reload.getAttribute("aria-busy")).toBe("true");
    expect(reload.textContent).toContain("Reconnecting…");
    expect(screen.getByText("Available after reconnection")).toBeTruthy();
    expect(screen.queryByText("Tools (0)")).toBeNull();
    expect(screen.getByRole("button", { name: "Remove" })).toHaveProperty("disabled", false);
    expect(view.postMessage.mock.calls.filter(([message]) => message.type === "reloadConnector")).toHaveLength(1);
    view.rerender({ ...snapshot, connectors: [...snapshot.connectors!] });
    expect(screen.getByTestId("connector-reload")).toHaveProperty("disabled", true);
    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    expect(screen.getByRole("button", { name: /playwright.*Reconnecting/ })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /other/i }));
    expect(screen.getByTestId("connector-reload")).toHaveProperty("disabled", false);
    fireEvent.click(screen.getByTestId("connector-reload"));
    expect(view.postMessage.mock.calls.filter(([message]) => message.type === "reloadConnector")).toHaveLength(2);
  });

  it("uses backend progress after reopening and keeps the card stable between attempts", () => {
    const snapshot = state(); const source = snapshot.connectors![0];
    Object.assign(source, { state: "connecting", generation: "2", attempt: 1, recovery: { phase: "starting", maxAttempts: 3, remainingMs: 50_000 } });
    const view = renderView(snapshot);
    const card = screen.getByRole("button", { name: /playwright.*Reconnecting/ });
    fireEvent.click(card);
    expect(within(screen.getByRole("dialog")).getByText("Reconnecting… 1/3").getAttribute("role")).toBe("status");
    expect(view.postMessage).toHaveBeenCalledWith(expect.objectContaining({ type: "listConnectorTools", data: { name: source.name, configKey: source.configKey } }));
    view.rerender({ ...snapshot, connectors: [{ ...source, attempt: 2 }] });
    expect(screen.getByRole("button", { name: /playwright.*Reconnecting/ })).toBe(card);
    expect(within(screen.getByRole("dialog")).getByText("Reconnecting… 2/3")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Close connector details" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    fireEvent.click(card);
    expect(screen.getByTestId("connector-reload")).toHaveProperty("disabled", true);
    expect(screen.queryByRole("switch", { name: "browser_click" })).toBeNull();
  });

  it.each(["failed", "unknown"] as const)("releases busy with an explicit %s outcome", (phase) => {
    const snapshot = state(); const source = snapshot.connectors![0]; const view = renderView(snapshot);
    fireEvent.click(screen.getByRole("button", { name: /playwright/i })); fireEvent.click(screen.getByTestId("connector-reload"));
    const request = view.postMessage.mock.calls.find(([message]) => message.type === "reloadConnector")![0];
    const message = phase === "failed" ? "initialize: transport request failed (HTTP 503)" : "Unable to confirm reconnection. Connection lost.";
    view.rerender({ ...snapshot, connectors: [{ ...source, state: phase === "failed" ? "failed" : "connected", attempt: 3 }],
      connectorReloads: { [source.configKey]: { configKey: source.configKey, requestId: request.messageId, phase, message } } });
    const dialog = within(screen.getByRole("dialog"));
    expect(dialog.getByTestId("connector-reload")).toHaveProperty("disabled", false);
    expect(dialog.getByTestId("connector-reload").getAttribute("aria-busy")).toBe("false");
    expect(dialog.getByText(phase === "failed" ? /Reconnect failed after 3 attempts/ : message)).toBeTruthy();
    if (phase === "unknown") expect(dialog.queryByText(/Reconnect failed/)).toBeNull();
  });

  it("registers a different recovering detail so the Host can deliver its new directory", () => {
    const snapshot = state();
    const source = { ...snapshot.connectors![0], state: "connecting" as const, generation: "2", attempt: 2, recovery: { phase: "starting", maxAttempts: 3, remainingMs: 50_000 } };
    snapshot.connectors = [source];
    snapshot.selectedConnector = "previous-source";
    snapshot.connectorToolsIdentity = { configKey: "previous-source", generation: "1", attempt: 1 };
    const view = renderView(snapshot);
    fireEvent.click(screen.getByRole("button", { name: /playwright.*Reconnecting/ }));
    expect(view.postMessage).toHaveBeenCalledWith(expect.objectContaining({ type: "listConnectorTools", data: { name: source.name, configKey: source.configKey } }));
    expect(screen.queryByRole("switch", { name: "browser_click" })).toBeNull();
    const connected = { ...snapshot, connectors: [{ ...source, state: "connected" as const, recovery: null }], selectedConnector: source.configKey, connectorToolsIdentity: null };
    view.rerender(connected);
    expect(screen.getByText("Loading tools…")).toBeTruthy();
    view.rerender({ ...connected, connectorToolsIdentity: { configKey: source.configKey, generation: "2", attempt: 2 } });
    expect(screen.getByRole("switch", { name: "browser_click" })).toBeTruthy();
    expect(view.postMessage.mock.calls.filter(([message]) => message.type === "listConnectorTools")).toHaveLength(1);
  });

  it("requires matching directory generation and attempt instead of exposing stale tools", () => {
    const snapshot = state(); const source = snapshot.connectors![0]; const view = renderView(snapshot);
    fireEvent.click(screen.getByRole("button", { name: /playwright/i }));
    expect(screen.getByRole("switch", { name: "browser_click" })).toBeTruthy();
    const next = { ...snapshot, connectors: [{ ...source, generation: "2", attempt: 2 }] };
    view.rerender(next);
    expect(screen.queryByRole("switch", { name: "browser_click" })).toBeNull();
    expect(screen.getByText("Loading tools…")).toBeTruthy();
    view.rerender({ ...next, connectorToolsIdentity: { configKey: source.configKey, generation: "2", attempt: 2 }, connectorTools: [{ ...snapshot.connectorTools![0], label: "fresh-tool" }] });
    expect(screen.getByRole("switch", { name: "fresh-tool" })).toBeTruthy();
    expect(view.postMessage.mock.calls.filter(([message]) => message.type === "listConnectorTools")).toHaveLength(1);
  });

  it("does not resurrect a closed or removed detail when later data arrives", () => {
    const snapshot = state(); const view = renderView(snapshot);
    fireEvent.click(screen.getByRole("button", { name: /playwright/i }));
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    view.rerender({ ...snapshot }); expect(screen.queryByRole("dialog")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /playwright/i }));
    view.rerender({ ...snapshot, connectors: [] }); expect(screen.queryByRole("dialog")).toBeNull();
    view.rerender(snapshot); expect(screen.queryByRole("dialog")).toBeNull();
  });

  it.each(["needs_authorization", "awaiting_project_trust"] as const)("exposes the %s next step rather than generic Reload", (status) => {
    const snapshot = state(); Object.assign(snapshot.connectors![0], { state: status, transport: "http", oauthConfigured: true });
    snapshot.connectorProject = { root: "/workspace/project", trusted: false };
    renderView(snapshot); fireEvent.click(screen.getByRole("button", { name: /playwright/i }));
    expect(screen.getByTestId("connector-reload")).toHaveProperty("disabled", true);
    expect(screen.getByTestId("connector-login")).toHaveProperty("disabled", status === "awaiting_project_trust");
    expect(within(screen.getByRole("dialog", { name: "Configure playwright" })).queryByText("Trust")).toBeNull();
  });

  it("shows one project action for two waiting services while Global stays connected", () => {
    const snapshot = state();
    snapshot.connectorProject = { root: "/workspace/long/project-name", trusted: false };
    snapshot.connectors!.push(
      { ...snapshot.connectors![0], name: "deepwiki", configKey: "project:deepwiki", source: "workspace", state: "awaiting_project_trust", transport: "http" },
      { ...snapshot.connectors![0], name: "browser", configKey: "project:browser", source: "workspace", state: "awaiting_project_trust" },
    );
    const view = renderView(snapshot);
    expect(screen.getByRole("heading", { name: /^connected$/i })).toBeTruthy();
    expect(screen.getByRole("heading", { name: /^awaiting project trust$/i })).toBeTruthy();
    expect(screen.getByText("/workspace/long/project-name")).toBeTruthy();
    fireEvent.click(screen.getByTestId("connector-trust-project"));
    fireEvent.click(screen.getByTestId("connector-trust-project"));
    const intents = view.postMessage.mock.calls.map(([intent]) => intent).filter((intent) => intent.type === "trustProject");
    expect(intents).toHaveLength(1);
    expect(intents[0].data).toEqual({ projectRoot: "/workspace/long/project-name" });
    fireEvent.click(screen.getByTestId("connector-card-deepwiki"));
    const dialog = within(screen.getByRole("dialog", { name: "Configure deepwiki" }));
    expect(dialog.queryByText("Trust")).toBeNull();
    expect(dialog.queryByText("Deny")).toBeNull();
    expect(dialog.getByText("Tools unavailable until connected")).toBeTruthy();
    fireEvent.click(dialog.getByRole("button", { name: "Done" }));
    view.rerender({ ...snapshot, connectorProject: { root: "/workspace/long/project-name", trusted: true }, connectors: snapshot.connectors!.map((connector) => ({ ...connector, state: "connected" })) });
    expect(screen.queryByTestId("connector-trust-project")).toBeNull();
    expect(screen.queryByRole("heading", { name: /^awaiting project trust$/i })).toBeNull();
  });

  it("uses Add and Trust for an untrusted Workspace but Add for Global, trusted and unknown", () => {
    const snapshot = state();
    snapshot.connectorProject = { root: "/workspace/project", trusted: false };
    snapshot.connectorConfigPaths = {
      global: { display: "~/.tomcat/mcp.json", raw: "/tmp/.tomcat/mcp.json" },
      workspace: { display: ".agents/mcp.json", raw: "/workspace/project/.agents/mcp.json" },
    };
    snapshot.connectors = [];
    const view = renderView(snapshot);
    fireEvent.click(screen.getByTestId("connector-add-open"));
    expect(screen.getByTestId("connector-add-submit").textContent).toBe("Add");
    fireEvent.click(screen.getByTestId("connector-scope-workspace"));
    expect(screen.getByTestId("connector-add-submit").textContent).toBe("Add and Trust");
    const form = within(screen.getByRole("dialog", { name: "Add Connector" }));
    expect(form.queryByText(/^Project$/)).toBeNull();
    expect(form.queryByRole("button", { name: "Trust project" })).toBeNull();
    fireEvent.click(screen.getByTestId("connector-add-submit"));
    expect(view.postMessage.mock.calls.filter(([message]) => message.type === "addConnector")).toHaveLength(0);
    fireEvent.change(screen.getByTestId("connector-name"), { target: { value: "browser" } });
    fireEvent.change(screen.getByTestId("connector-command"), { target: { value: "node" } });
    fireEvent.click(screen.getByTestId("connector-add-submit"));
    fireEvent.click(screen.getByTestId("connector-add-submit"));
    const adds = view.postMessage.mock.calls.map(([message]) => message).filter((message) => message.type === "addConnector");
    expect(adds).toHaveLength(1);
    expect(adds[0].data).toMatchObject({ connector: { name: "browser", scope: "workspace" }, trustProject: true });
    expect(adds[0].data.connector).not.toHaveProperty("trustProject");
    expect(screen.getByTestId("connector-add-submit")).toHaveProperty("disabled", true);
    view.rerender({ ...snapshot, connectorReceipt: { requestId: adds[0].messageId, configSaved: false, connectionStarted: false, error: "try again" } });
    expect(screen.getByText("try again")).toBeTruthy();
    fireEvent.click(screen.getByTestId("connector-scope-global"));
    expect(screen.getByTestId("connector-add-submit").textContent).toBe("Add");
    fireEvent.click(screen.getByTestId("connector-scope-workspace"));
    view.rerender({ ...snapshot, connectorProject: null });
    expect(screen.getByTestId("connector-add-submit").textContent).toBe("Add");
    view.rerender({ ...snapshot, connectorProject: { root: "/workspace/project", trusted: true } });
    expect(screen.getByTestId("connector-add-submit").textContent).toBe("Add");
  });

  it("explains that project resource directory changes require a restart", () => {
    renderView();

    expect(
      screen
        .getByText("Changes to the project resource directory take effect after restarting Tomcat.")
        .getAttribute("role"),
    ).toBe("status");
  });

  it("explains the saved configuration and connection without transport jargon", async () => {
    const snapshot = state();
    snapshot.connectors![0].toolCount = 1;
    const { container, postMessage } = renderView(snapshot);

    expect(screen.getByTestId("connector-card-playwright").textContent).toContain("Connected · 1 enabled tools · stdio");
    fireEvent.click(screen.getByRole("button", { name: /playwright/i }));

    await screen.findByText("Tools (2) · 1 enabled");
    expect(screen.getByText("Config file")).toBeTruthy();
    expect(screen.getByText("~/.tomcat/mcp.json")).toBeTruthy();
    expect(screen.getByText("Connection")).toBeTruthy();
    expect(screen.queryByText("Transport")).toBeNull();
    expect(container.querySelector(".tc-connector-tool__indicator--enabled")).toBeTruthy();
    expect(container.querySelectorAll(".tc-connector-tool__indicator")).toHaveLength(2);
    const configLink = container.querySelector(".tc-connector-config-link");
    expect(configLink?.classList.contains("tc-inline-path")).toBe(true);
    expect(container.querySelector(".tc-connector-config-link code")).toBeNull();
    expect(container.querySelector(".tc-connector-config-link .codicon-file")?.getAttribute("aria-hidden")).toBe("true");
    const closeTool = screen.getByRole("switch", { name: "browser_close" });
    closeTool.focus();
    fireEvent.click(closeTool);
    expect(postMessage).toHaveBeenCalledWith(expect.objectContaining({
      type: "setConnectorToolEnabled",
      data: {
        configKey: "mcp:/tmp/.tomcat/mcp.json:playwright",
        enabled: true,
        name: "playwright",
        rawName: "browser_close",
      },
    }));
    expect(screen.getByText("browser_close")).toBeTruthy();
    expect(document.activeElement).toBe(closeTool);
    expect(screen.getByRole("switch", { name: "browser_close" }).getAttribute("aria-disabled")).toBe("true");
    expect(screen.getByRole("switch", { name: "browser_click" }).getAttribute("aria-disabled")).toBe("true");
    fireEvent.click(screen.getByRole("switch", { name: "browser_click" }));
    fireEvent.click(screen.getByRole("switch", { name: "browser_close" }));
    expect(postMessage.mock.calls.filter(([message]) => message.type === "setConnectorToolEnabled")).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "~/.tomcat/mcp.json" }));
    expect(postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        data: { configKey: "mcp:/tmp/.tomcat/mcp.json:playwright" },
        type: "openConnectorConfig",
      }),
    );
  });

  it("releases a matching tool save and links a batch-rule conflict to its configuration", async () => {
    const snapshot = state();
    const view = renderView(snapshot);
    const source = snapshot.connectors![0];
    fireEvent.click(screen.getByRole("button", { name: /playwright/i }));
    fireEvent.click(screen.getByRole("switch", { name: "browser_click" }));
    const request = view.postMessage.mock.calls.find(([message]) => message.type === "setConnectorToolEnabled")![0];
    const key = JSON.stringify([source.configKey, "browser_click"]);

    view.rerender({
      ...snapshot,
      connectorToolToggles: {
        [key]: {
          requestId: "late-reply",
          configKey: source.configKey,
          rawName: "browser_click",
          enabled: false,
          configSaved: true,
          runtimeApplied: true,
        },
      },
    });
    expect(screen.getByRole("switch", { name: "browser_click" }).getAttribute("aria-disabled")).toBe("true");

    view.rerender({
      ...snapshot,
      connectorToolToggles: {
        [key]: {
          requestId: request.messageId,
          configKey: source.configKey,
          rawName: "browser_click",
          enabled: false,
          configSaved: false,
          runtimeApplied: false,
          error: upstreamToggleError,
        },
      },
    });

    expect(await screen.findByText(upstreamToggleError)).toBeTruthy();
    const restoredTool = await screen.findByRole("switch", { name: "browser_click" });
    expect(restoredTool.getAttribute("aria-disabled")).toBe("false");
    expect(screen.getAllByRole("button", { name: "~/.tomcat/mcp.json" })).toHaveLength(2);
  });

  it("ends a disconnected save without leaving the restored switch busy", async () => {
    const snapshot = state();
    const view = renderView(snapshot);
    const source = snapshot.connectors![0];
    fireEvent.click(screen.getByRole("button", { name: /playwright/i }));
    fireEvent.click(screen.getByRole("switch", { name: "browser_click" }));
    const request = view.postMessage.mock.calls.find(([message]) => message.type === "setConnectorToolEnabled")![0];
    const key = JSON.stringify([source.configKey, "browser_click"]);

    view.rerender({ ...snapshot, ready: false, connectorTools: [], connectorToolsIdentity: null });
    await waitFor(() => expect(screen.queryByRole("switch", { name: "browser_click" })).toBeNull());
    view.rerender({
      ...snapshot,
      connectorToolToggles: {
        [key]: {
          requestId: request.messageId,
          configKey: source.configKey,
          rawName: "browser_click",
          enabled: false,
          error: "Connection lost; result unknown.",
        },
      },
    });

    const restored = await screen.findByRole("switch", { name: "browser_click" });
    expect(restored.getAttribute("aria-busy")).toBe("false");
    expect(restored.getAttribute("aria-disabled")).toBe("true");
    expect(screen.getByText("Connection lost; result unknown.")).toBeTruthy();
  });

  it("retries the original target after a partial result even when the catalog is temporarily unavailable", async () => {
    const snapshot = state();
    const view = renderView(snapshot);
    const source = snapshot.connectors![0];
    fireEvent.click(screen.getByRole("button", { name: /playwright/i }));
    fireEvent.click(screen.getByRole("switch", { name: "browser_click" }));
    const request = view.postMessage.mock.calls.find(([message]) => message.type === "setConnectorToolEnabled")![0];
    const key = JSON.stringify([source.configKey, "browser_click"]);
    view.rerender({
      ...snapshot,
      connectorTools: [],
      connectorToolsIdentity: null,
      connectorToolToggles: {
        [key]: {
          requestId: request.messageId,
          configKey: source.configKey,
          rawName: "browser_click",
          enabled: false,
          configSaved: true,
          runtimeApplied: false,
          error: "Runtime synchronization could not be confirmed.",
        },
      },
    });

    const retry = await screen.findByRole("button", { name: "Retry" });
    await waitFor(() => expect(retry).toHaveProperty("disabled", false));
    fireEvent.click(retry);
    const retries = view.postMessage.mock.calls.filter(([message]) => message.type === "setConnectorToolEnabled");
    expect(retries).toHaveLength(2);
    expect(retries[1][0]).toMatchObject({ data: { configKey: source.configKey, rawName: "browser_click", enabled: false } });
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

  it("shows overridden Global connectors without attempting tool or connection actions", async () => {
    const snapshot = state();
    snapshot.connectors![0].overridden = true;
    const { postMessage } = renderView(snapshot);

    fireEvent.click(screen.getByRole("button", { name: /playwright/i }));

    expect(screen.getByText(/overridden by the same-named Workspace connector/i)).toBeTruthy();
    expect(screen.getByRole("button", { name: "↻ Reload" })).toHaveProperty("disabled", true);
    expect(postMessage).not.toHaveBeenCalledWith(
      expect.objectContaining({ type: "listConnectorTools" }),
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
        data: { scope: "global" },
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
