import * as assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as http from "node:http";
import * as path from "node:path";

const repoRoot = path.resolve(__dirname, "../../../");
const hostE2e = require(path.resolve(
  repoRoot,
  "out/test/suite/support/hostE2eScenario.js",
)) as {
  getTomcatExtensionApi(): Promise<InstalledConnectorApi>;
};
const workbenchCapture = require(path.resolve(
  repoRoot,
  "out/test/suite/support/workbenchFindDriver.js",
)) as {
  captureWorkbenchArtifacts(target: string): Promise<void>;
};

type Connector = {
  configKey: string;
  name: string;
  source: "global" | "workspace";
  oauthConfigured: boolean;
  state: string;
  toolCount: number;
};

type InstalledConnectorApi = {
  __testing: {
    captureSettingsDom(): Promise<{ html: string }>;
    executeCommand(command: string, ...args: unknown[]): Thenable<unknown>;
    getObservedWebviewErrors(): Array<{ message: string; stack?: string }>;
    getSettingsPanelState(): {
      route: "connectors" | "models";
      state: {
        connectorTools?: Array<{ rawName: string }>;
        connectors?: Connector[];
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

function requiredEnv(name: string): string {
  const value = process.env[name];
  assert.ok(value, `expected ${name} to be set`);
  return value;
}

async function pause(ms: number): Promise<void> {
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

async function waitFor<T>(description: string, predicate: () => T | undefined): Promise<T> {
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

suite("Installed real-Serve connector acceptance", () => {
  test("adds, connects, and renders a controlled stdio MCP connector", async () => {
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
    // The test bridge delivers events asynchronously; let React commit the scope
    // selection before the next action captures the submit handler closure.
    await pause(200);
    await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "connector-add-submit" });

    const added = await waitFor("saved controlled connector", () => api
      .__testing
      .getSettingsPanelState()
      .state
      .connectors
      ?.find((connector) => connector.name === "installed-controlled-stdio"));
    assert.equal(added.source, "workspace", "the installation test must exercise Workspace trust");
    // Adding a Workspace connector is an explicit user approval, so the server
    // records trust before its background connection attempt; it must not re-prompt.

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
    await fs.writeFile(path.join(artifactsRoot, "connectors-installed.aria.txt"), dom.html, "utf8");
    await workbenchCapture.captureWorkbenchArtifacts(
      path.join(artifactsRoot, "connectors-installed.png"),
    );
    // Exercise a controlled OAuth discovery/registration flow, then cancel the live login.
    const oauthService = await startControlledOAuthService();
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
    await api.__testing.sendSettingsDomAction({ kind: "clickTestId", testId: "connector-cancel-login" });
    await waitFor("OAuth cancellation terminal state", () => api
      .__testing
      .getSettingsPanelState()
      .state
      .connectors
      ?.find((connector) => connector.configKey === oauth.configKey && connector.state === "disconnected"));
    assert.ok(oauth.oauthConfigured, "OAuth form selection must persist to the connector card");

    await fs.writeFile(
      path.join(artifactsRoot, "controlled-oauth-requests.json"),
      `${JSON.stringify(oauthService.requests, null, 2)}\n`,
      "utf8",
    );
    await oauthService.close();

    await fs.writeFile(
      path.join(artifactsRoot, "connectors-installed-console.json"),
      `${JSON.stringify(api.__testing.getObservedWebviewErrors(), null, 2)}\n`,
      "utf8",
    );
    assert.deepEqual(api.__testing.getObservedWebviewErrors(), []);
  });
});
