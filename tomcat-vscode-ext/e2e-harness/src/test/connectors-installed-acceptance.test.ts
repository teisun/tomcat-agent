import * as assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as http from "node:http";
import * as path from "node:path";

export const repoRoot = path.resolve(__dirname, "../../../");
export const hostE2e = require(path.resolve(
  repoRoot,
  "out/test/suite/support/hostE2eScenario.js",
)) as {
  getTomcatExtensionApi(): Promise<InstalledConnectorApi>;
};
export const workbenchCapture = require(path.resolve(
  repoRoot,
  "out/test/suite/support/workbenchFindDriver.js",
)) as {
  SettingsFrameDriver: { connectFromEnvironment(): Promise<SettingsDriver> };
};

export type SettingsDom = {
  text: string;
  viewport: { width: number; height: number };
  buttons: Array<{ testId: string | null; name: string; disabled: boolean; busy: string | null; width: number; height: number }>;
  feedback: Array<{ text: string; width: number; scrollWidth: number }>;
};
export type SettingsDriver = {
  capture(target: string): Promise<SettingsDom>;
  evaluate<T>(expression: string): Promise<T>;
  setViewport(width: number, height: number): Promise<void>;
  focusAndPress(testId: string, key: "Enter" | "Escape"): Promise<void>;
  hover(testId: string): Promise<void>;
  snapshot(): Promise<SettingsDom>;
  waitForSnapshot(predicate: (value: SettingsDom) => boolean, description: string): Promise<SettingsDom>;
  close(): void;
};

export type Connector = {
  configKey: string;
  name: string;
  source: "global" | "workspace";
  oauthConfigured: boolean;
  state: string;
  toolCount: number;
  generation?: string;
  attempt?: number;
  recovery?: { phase: string; attempt: number; maxAttempts: number } | null;
  error?: string | null;
};

export type InstalledConnectorApi = {
  __testing: {
    captureSettingsDom(): Promise<{ html: string }>;
    executeCommand(command: string, ...args: unknown[]): Thenable<unknown>;
    getObservedWebviewErrors(): Array<{ message: string; stack?: string }>;
    getResolvedExecutable(): { executable: string; found: boolean };
    getSettingsPanelState(): {
      route: "connectors" | "models";
      state: {
        connectorTools?: Array<{ rawName: string; enabled: boolean }>;
        connectorToolToggles?: Record<string, {
          configSaved?: boolean;
          enabled: boolean;
          error?: string | null;
          rawName: string;
          runtimeApplied?: boolean;
        }>;
        connectorReloads?: Record<string, { phase: string; generation?: string; requestId: string }>;
        connectorToolsIdentity?: { configKey: string; generation: string; attempt: number } | null;
        connectors?: Connector[];
        connectorConfigPaths?: { global: string; workspace?: string | null };
        connectorProject?: { root: string; trusted: boolean } | null;
        models: unknown[];
        ready: boolean;
        selectedConnector?: string | null;
      };
      visible: boolean;
      webviewReady: boolean;
    };
    getWebviewState(): { ready: boolean };
    sendSettingsDomAction(action: {
      kind: "clickTestId" | "setInputValue";
      testId: string;
      value?: string;
    }): Promise<void>;
    sendSettingsIntent(intent: {
      data?: Record<string, unknown>;
      messageId: string;
      type: string;
    }): Promise<void>;
    waitForWebviewReady(timeoutMs?: number): Promise<void>;
  };
};

export function requiredEnv(name: string): string {
  const value = process.env[name];
  assert.ok(value, `expected ${name} to be set`);
  return value;
}

export async function pause(ms: number): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, ms));
}

type ControlledOAuthService = {
  baseUrl: string;
  close(): Promise<void>;
  requests: string[];
};

async function startControlledOAuthService(): Promise<ControlledOAuthService> {
  const requests: string[] = [];
  let baseUrl = "";
  const server = http.createServer((request, response) => {
    const requestUrl = new URL(request.url ?? "/", baseUrl);
    requests.push(requestUrl.pathname);
    const json = (payload: unknown, status = 200) => {
      response.writeHead(status, { "content-type": "application/json" });
      response.end(JSON.stringify(payload));
    };
    switch (requestUrl.pathname) {
      case "/mcp":
        response.writeHead(401, {
          "www-authenticate": `Bearer resource_metadata="${baseUrl}/.well-known/oauth-protected-resource"`,
        });
        response.end();
        return;
      case "/.well-known/oauth-protected-resource":
        json({ authorization_servers: [baseUrl], resource: `${baseUrl}/mcp` });
        return;
      case "/.well-known/oauth-authorization-server":
        json({
          authorization_endpoint: `${baseUrl}/authorize`,
          issuer: baseUrl,
          registration_endpoint: `${baseUrl}/register`,
          scopes_supported: ["mcp.read"],
          token_endpoint: `${baseUrl}/token`,
        });
        return;
      case "/register":
        json({ client_id: "controlled-acceptance-client" });
        return;
      case "/authorize":
        response.writeHead(200, { "content-type": "text/html" });
        response.end("<title>Controlled OAuth authorization</title>");
        return;
      case "/token":
        json({ access_token: "controlled-acceptance-token", expires_in: 60 });
        return;
      default:
        response.writeHead(404);
        response.end();
    }
  });
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  assert.ok(address && typeof address !== "string", "controlled OAuth service must bind a TCP port");
  baseUrl = `http://127.0.0.1:${address.port}`;
  return {
    baseUrl,
    requests,
    close: () => new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve())),
  };
}

export async function waitFor<T>(description: string, predicate: () => T | undefined): Promise<T> {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    const value = predicate();
    if (value !== undefined) {
      return value;
    }
    await pause(100);
  }
  throw new Error(`Timed out waiting for ${description}`);
}

let serveStarted: Promise<void> | undefined;
export function handleInitialProjectPrompt(): Promise<void> {
  serveStarted ??= (async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    // Launch Serve before Settings. VS Code's --extensionTestsPath host refuses
    // showWarningMessage({ modal: true }) outright, so it cannot visually test
    // this native dialog. Keep the fixture untrusted and cover the Settings
    // button / Add and Trust here; verify the first-load dialog in a real host.
    await api.__testing.executeCommand("tomcat.session.list");
    const artifacts = requiredEnv("TOMCAT_CONNECTORS_ACCEPT_ARTIFACTS_DIR");
    await fs.writeFile(path.join(artifacts, "project-trust-test-host-limit.txt"),
      "VS Code --extensionTestsPath refuses modal dialogs; no first-load PNG/ARIA is claimed by this fixture.\n");
  })();
  return serveStarted;
}

suite("Installed real-Serve connector acceptance", () => {
  suiteSetup(async function () { this.timeout(80_000); await handleInitialProjectPrompt(); });
  let oauthCleanup: (() => Promise<void>) | undefined;
  teardown(async () => { await oauthCleanup?.(); oauthCleanup = undefined; });
  (process.env.TOMCAT_CONNECTORS_ACCEPT_PHASE === "untrusted" ? test : test.skip)("groups two waiting project servers behind one button and keeps Global connected", async function () {
    this.timeout(90_000);
    const api = await hostE2e.getTomcatExtensionApi();
    await api.__testing.executeCommand("tomcat.openSettings");
    await waitFor("Settings ready", () => api.__testing.getSettingsPanelState().webviewReady ? true : undefined);
    await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "settings-nav-connectors" });
    const driver = await workbenchCapture.SettingsFrameDriver.connectFromEnvironment();
    const artifacts = requiredEnv("TOMCAT_CONNECTORS_ACCEPT_ARTIFACTS_DIR");
    try {
      let before: ReturnType<InstalledConnectorApi["__testing"]["getSettingsPanelState"]>["state"];
      try {
        await waitFor("two waiting project servers and an unaffected Global source", () => {
          const state = api.__testing.getSettingsPanelState().state;
          return state.connectors?.filter((entry) => entry.state === "awaiting_project_trust").length === 2
            && state.connectors.some((entry) => entry.name === "always-global" && entry.source === "global")
            && state.connectorProject?.trusted === false ? state : undefined;
        });
        // This clean profile has no chat session yet. An explicit Reload proves
        // that Global can connect while the two Workspace sources remain gated.
        await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "connector-card-always-global" });
        await driver.waitForSnapshot((dom) => dom.buttons.some((button) => button.testId === "connector-reload" && !button.disabled), "Global remains reloadable without project trust");
        await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "connector-reload" });
        before = await waitFor("Global connected without trusting the project", () => {
          const state = api.__testing.getSettingsPanelState().state;
          return state.connectors?.some((entry) => entry.name === "always-global" && entry.state === "connected")
            && state.connectors.filter((entry) => entry.state === "awaiting_project_trust").length === 2 ? state : undefined;
        });
        await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "connector-detail-done" });
      } catch (error) {
        await fs.writeFile(path.join(artifacts, "project-trust-awaiting-failure.json"), JSON.stringify(api.__testing.getSettingsPanelState(), null, 2));
        throw error;
      }
      await driver.waitForSnapshot((dom) => dom.buttons.filter((button) => button.testId === "connector-trust-project").length === 1, "one project trust button");
      await driver.capture(path.join(artifacts, "project-trust-awaiting.png"));
      await driver.setViewport(390, 844);
      // VS Code webview iframe layout update can lag behind main window resize
      await driver.waitForSnapshot((dom) => dom.viewport.width > 0 && dom.viewport.width <= 400, "viewport adjusted to narrow");
      const narrow = await driver.capture(path.join(artifacts, "project-trust-awaiting-narrow.png"));
      assert.equal(narrow.buttons.filter((button) => button.testId === "connector-trust-project" && button.width > 0).length, 1);
      await driver.setViewport(1440, 900);
      assert.ok(before.connectorProject?.root);
      await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "connector-card-project-one" });
      await driver.waitForSnapshot((dom) => dom.buttons.some((button) => button.testId === "connector-reload" && button.disabled), "waiting project detail");
      const detail = await driver.capture(path.join(artifacts, "project-trust-detail.png"));
      assert.ok(!detail.buttons.some((button) => /^(Trust|Deny)$/.test(button.name)), "service details must not request per-server approval");
      await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "connector-detail-done" });
      await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "connector-trust-project" });
      await waitFor("both project servers connected after one approval", () => {
        const state = api.__testing.getSettingsPanelState().state;
        return state.connectorProject?.trusted === true && ["project-one", "project-two"].every((name) => state.connectors?.some((entry) => entry.name === name && entry.state === "connected")) ? true : undefined;
      });
      const approved = await driver.capture(path.join(artifacts, "project-trust-approved.png"));
      assert.ok(!approved.buttons.some((button) => button.testId === "connector-trust-project"));
    } finally { driver.close(); }
  });

  (process.env.TOMCAT_CONNECTORS_ACCEPT_PHASE === "add-and-trust" ? test : test.skip)("adds an untrusted Workspace source with one Add and Trust action", async function () {
    this.timeout(90_000);
    const api = await hostE2e.getTomcatExtensionApi();
    await api.__testing.executeCommand("tomcat.openSettings");
    await waitFor("Settings ready", () => api.__testing.getSettingsPanelState().webviewReady ? true : undefined);
    await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "settings-nav-connectors" });
    const driver = await workbenchCapture.SettingsFrameDriver.connectFromEnvironment();
    const artifacts = requiredEnv("TOMCAT_CONNECTORS_ACCEPT_ARTIFACTS_DIR");
    try {
      await waitFor("Global usable while project is untrusted", () => {
        const state = api.__testing.getSettingsPanelState().state;
        return state.connectorProject?.trusted === false && state.connectors?.some((entry) => entry.name === "always-global" && entry.state === "connected") ? true : undefined;
      });
      await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "connector-add-open" });
      await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "connector-scope-workspace" });
      await driver.waitForSnapshot((dom) => dom.buttons.some((button) => button.testId === "connector-add-submit" && button.name === "Add and Trust"), "untrusted Workspace submit label");
      const fields = await driver.evaluate<string[]>(`[...document.querySelectorAll('[role="dialog"] label')].map(node => node.textContent.trim())`);
      assert.ok(!fields.some((label) => /^Project$/.test(label)), "Add form must not add a project trust row");
      await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "connector-transport-http" });
      await driver.capture(path.join(artifacts, "project-add-and-trust-http.png"));
      await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "connector-transport-stdio" });
      const form = await driver.capture(path.join(artifacts, "project-add-and-trust-form.png"));
      assert.ok(!form.buttons.some((button) => button.name === "Trust project"), "no separate Trust button in Add form");
      await api.__testing.sendSettingsDomAction({ kind: "setInputValue", testId: "connector-name", value: "added-once" });
      await api.__testing.sendSettingsDomAction({ kind: "setInputValue", testId: "connector-command", value: "node" });
      await api.__testing.sendSettingsDomAction({ kind: "setInputValue", testId: "connector-args", value: path.resolve(repoRoot, "../tomcat/tests/fixtures/mcp/fake_stdio_server.mjs") });
      await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "connector-add-submit" });
      await waitFor("Add and Trust connects the new project source", () => {
        const state = api.__testing.getSettingsPanelState().state;
        return state.connectorProject?.trusted === true && state.connectors?.some((entry) => entry.name === "added-once" && entry.state === "connected") ? true : undefined;
      });
      await driver.capture(path.join(artifacts, "project-add-and-trust-connected.png"));
    } finally { driver.close(); }
  });

  (process.env.TOMCAT_CONNECTORS_ACCEPT_PHASE === "normal" ? test : test.skip)("adds controlled stdio and cancels an OAuth login", async () => {
    const api = await hostE2e.getTomcatExtensionApi();
    const fixture = path.resolve(
      repoRoot,
      "../tomcat/tests/fixtures/mcp/fake_stdio_server.mjs",
    );
    await fs.access(fixture);
    await api.__testing.executeCommand("tomcat.openSettings");

    await waitFor("settings panel", () => {
      const state = api.__testing.getSettingsPanelState();
      return state.visible && state.webviewReady ? state : undefined;
    });
    await api.__testing.sendSettingsDomAction({
      kind: "clickTestId",
      testId: "settings-nav-connectors",
    });
    await waitFor("connector route", () => {
      const state = api.__testing.getSettingsPanelState();
      return state.route === "connectors" ? state : undefined;
    });
    await waitFor("Workspace connector configuration available", () => {
      const state = api.__testing.getSettingsPanelState().state;
      return state.connectorConfigPaths?.workspace && state.connectorProject?.root
        && state.connectorProject.trusted === false ? state : undefined;
    });

    // Exercise the rendered form rather than injecting an addConnector frame.
    await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "connector-add-open" });
    await api.__testing.sendSettingsDomAction({
      kind: "setInputValue",
      testId: "connector-name",
      value: "installed-controlled-stdio",
    });
    await api.__testing.sendSettingsDomAction({
      kind: "setInputValue",
      testId: "connector-command",
      // process.execPath is VS Code's Electron helper in an extension host, not Node.
      value: "node",
    });
    await api.__testing.sendSettingsDomAction({
      kind: "setInputValue",
      testId: "connector-args",
      value: fixture,
    });
    await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "connector-scope-workspace" });
    const formDriver = await workbenchCapture.SettingsFrameDriver.connectFromEnvironment();
    try {
      await formDriver.waitForSnapshot((dom) => dom.buttons.some((button) => button.testId === "connector-add-submit" && button.name === "Add and Trust"), "Workspace Add and Trust selection committed");
      assert.equal(await formDriver.evaluate<boolean>(`document.querySelector('[data-testid="connector-scope-workspace"]')?.checked === true`), true);
    } finally { formDriver.close(); }
    await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "connector-add-submit" });

    const added = await waitFor("saved controlled connector", () => api
      .__testing
      .getSettingsPanelState()
      .state
      .connectors
      ?.find((connector) => connector.name === "installed-controlled-stdio"));
    assert.equal(added.source, "workspace", "the installation test must exercise Workspace trust");
    // This phase enters through Add and Trust: the extension test host refuses
    // native first-load dialogs, and Add must still work without one.
    const connected = await waitFor("controlled connector connection", () => api
      .__testing
      .getSettingsPanelState()
      .state
      .connectors
      ?.find((connector) => connector.configKey === added.configKey && connector.state === "connected"));
    assert.ok(connected.toolCount > 0, "connected MCP must report its discovered tools");
    await api.__testing.sendSettingsDomAction({
      kind: "clickTestId",
      testId: "connector-card-installed-controlled-stdio",
    });
    await waitFor("controlled connector tools", () => {
      const state = api.__testing.getSettingsPanelState().state;
      return state.selectedConnector === added.configKey
        && state.connectorTools?.some((tool) => tool.rawName === "capture")
        ? state
        : undefined;
    });

    const dom = await api.__testing.captureSettingsDom();
    assert.ok(dom.html.includes("installed-controlled-stdio"));
    assert.ok(dom.html.includes("capture"));
    const artifactsRoot = requiredEnv("TOMCAT_CONNECTORS_ACCEPT_ARTIFACTS_DIR");
    await fs.writeFile(path.join(artifactsRoot, "connectors-installed.dom.html"), dom.html, "utf8");
    const settings = await workbenchCapture.SettingsFrameDriver.connectFromEnvironment();
    try { await settings.capture(path.join(artifactsRoot, "connectors-installed.png")); }
    finally { settings.close(); }
    // Exercise a controlled OAuth discovery/registration flow, then cancel the live login.
    const oauthService = await startControlledOAuthService();
    oauthCleanup = () => oauthService.close();
    await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "connector-detail-done" });
    await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "connector-add-open" });
    await api.__testing.sendSettingsDomAction({
      kind: "setInputValue",
      testId: "connector-name",
      value: "installed-controlled-oauth",
    });
    await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "connector-transport-http" });
    await api.__testing.sendSettingsDomAction({
      kind: "setInputValue",
      testId: "connector-url",
      value: `${oauthService.baseUrl}/mcp`,
    });
    await api.__testing.sendSettingsDomAction({
      kind: "setInputValue",
      testId: "connector-auth",
      value: "oauth",
    });
    await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "connector-add-submit" });
    const oauth = await waitFor("saved OAuth connector", () => api
      .__testing
      .getSettingsPanelState()
      .state
      .connectors
      ?.find((connector) => connector.name === "installed-controlled-oauth"));
    await api.__testing.sendSettingsDomAction({
      kind: "clickTestId",
      testId: "connector-card-installed-controlled-oauth",
    });
    let oauthDetail = "";
    for (let attempt = 0; attempt < 100; attempt += 1) {
      oauthDetail = (await api.__testing.captureSettingsDom()).html;
      if (oauthDetail.includes("OAuth 2.0")) break;
      await pause(100);
    }
    assert.ok(oauthDetail.includes("OAuth 2.0"), "OAuth connector detail must render its selected authentication");
    await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "connector-login" });
    await waitFor("controlled OAuth discovery and registration", () =>
      oauthService.requests.includes("/.well-known/oauth-protected-resource")
      && oauthService.requests.includes("/.well-known/oauth-authorization-server")
      && oauthService.requests.includes("/register")
        ? true
        : undefined);
    const oauthSettings = await workbenchCapture.SettingsFrameDriver.connectFromEnvironment();
    try {
      await oauthSettings.waitForSnapshot((snapshot) => snapshot.buttons.some((button) => button.testId === "connector-cancel-login" && !button.disabled && button.width > 0), "OAuth Cancel is rendered");
      await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "connector-cancel-login" });
      // Cancel is not Logout: a source that still needs credentials must retain
      // NeedsAuthorization rather than being falsely relabelled Disconnected.
      await waitFor("OAuth cancellation terminal state", () => api.__testing.getSettingsPanelState().state.connectors?.find((connector) => connector.configKey === oauth.configKey && connector.state === "needs_authorization"));
      await oauthSettings.waitForSnapshot((snapshot) => !snapshot.buttons.some((button) => button.testId === "connector-cancel-login" && button.width > 0) && snapshot.buttons.some((button) => button.testId === "connector-login" && !button.disabled), "OAuth cancellation returns the Login action");
      await oauthSettings.capture(path.join(artifactsRoot, "oauth-cancelled.png"));
    } catch (error) {
      await fs.writeFile(path.join(artifactsRoot, "oauth-cancellation-failure.json"), JSON.stringify(api.__testing.getSettingsPanelState(), null, 2));
      throw error;
    } finally { oauthSettings.close(); }
    assert.ok(oauth.oauthConfigured, "OAuth form selection must persist to the connector card");

    await fs.writeFile(
      path.join(artifactsRoot, "controlled-oauth-requests.json"),
      `${JSON.stringify(oauthService.requests, null, 2)}\n`,
      "utf8",
    );
    await oauthCleanup?.();
    oauthCleanup = undefined;

    await fs.writeFile(
      path.join(artifactsRoot, "connectors-installed-console.json"),
      `${JSON.stringify(api.__testing.getObservedWebviewErrors(), null, 2)}\n`,
      "utf8",
    );
    assert.deepEqual(api.__testing.getObservedWebviewErrors(), []);
  });
});
