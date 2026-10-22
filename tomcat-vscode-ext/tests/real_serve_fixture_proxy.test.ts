import * as path from "node:path";
import { mkdir, readFile, stat, writeFile } from "node:fs/promises";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setupRealServeFixture } from "../scripts/realServeFixture";
import { windowTestProxy } from "../scripts/vscodeLaunchEnv";
import { TomcatMessenger } from "../src/serveClient/TomcatMessenger";
import { initializeServe } from "../src/serveClient/initialize";
import { ensureTomcatBinary, setupServeFixture, spawnScriptedOpenAiStreamServer, sseDelta, sseFinish, sseDone, waitForEvent } from "./serveTestUtils";

afterEach(() => vi.unstubAllEnvs());

describe("window proxy input and isolated configuration", () => {
  it("requires one explicit selection, never falls back, and prefers explicit test configuration", () => {
    const env = { HTTPS_PROXY: "http://window.invalid:8080", HTTP_PROXY: "http://other.invalid:8080", NO_PROXY: "*" };
    const before = { ...env };
    expect(windowTestProxy(env)).toBeUndefined();
    expect(windowTestProxy({ ...env, TOMCAT_E2E_WINDOW_PROXY_ENV: "HTTPS_PROXY" })).toBe(env.HTTPS_PROXY);
    expect(windowTestProxy({ ...env, TOMCAT_E2E_WINDOW_PROXY_ENV: "all_proxy" })).toBeUndefined();
    expect(windowTestProxy({ ...env, TOMCAT_E2E_WINDOW_PROXY_ENV: "HTTPS_PROXY", TOMCAT_E2E_LLM_PROXY: "http://configured.invalid:8080" }))
      .toBe("http://configured.invalid:8080");
    expect(() => windowTestProxy({ ...env, TOMCAT_E2E_WINDOW_PROXY_ENV: "NO_PROXY" })).toThrow("must select one");
    expect(env).toEqual(before);
  });

  it.each(["", "not a URL", "http://user:PRIVATE_PROXY_SENTINEL@proxy.invalid:bad-port", "file:///tmp/not-a-proxy"])(
    "rejects invalid explicit configuration before creating a fixture without echoing credentials", async llmProxy => {
      const runCli = vi.fn(async () => undefined);
      const result = setupRealServeFixture("http://fixture.invalid", "openai", { binary: "not-executed", llmProxy, runCli });
      await expect(result).rejects.toThrow("Test llm.proxy must be an absolute HTTP(S) proxy URL");
      await expect(result).rejects.not.toThrow("PRIVATE_PROXY_SENTINEL");
      expect(runCli).not.toHaveBeenCalled();
    },
  );

  it("only writes a supplied proxy to the new home; generic/ACP setup ignores even test-launcher inputs", async () => {
    vi.stubEnv("HTTP_PROXY", "http://host.invalid:7890");
    vi.stubEnv("HTTPS_PROXY", "http://user:p%24word@window.invalid:7890");
    vi.stubEnv("TOMCAT_E2E_WINDOW_PROXY_ENV", "HTTPS_PROXY");
    vi.stubEnv("TOMCAT_E2E_LLM_PROXY", undefined);
    const original = "[llm]\n# proxy = \"http://127.0.0.1:7890\"\ndefault_model = \"fixture\"\n[agent]\nid = \"main\"\n";
    const selected = windowTestProxy(process.env);
    for (const llmProxy of [undefined, selected]) {
      const runCli = vi.fn(async (_binary, args, { env, cwd }) => {
        expect(cwd).toBe(path.join(env.HOME!, "workspace"));
        expect(env.HTTP_PROXY).toBe("");
        expect(env.HTTPS_PROXY).toBe("");
        if (args[0] === "init") {
          await mkdir(path.join(env.HOME!, ".tomcat"));
          await writeFile(path.join(env.HOME!, ".tomcat", "tomcat.config.toml"), original);
        }
      });
      const fixture = await setupRealServeFixture("http://fixture.invalid", "openai", { binary: "not-executed", llmProxy, runCli });
      try {
        expect(fixture.homePath).not.toBe(process.env.HOME);
        const saved = await readFile(path.join(fixture.homePath, ".tomcat", "tomcat.config.toml"), "utf8");
        expect(saved).toBe(llmProxy === undefined ? original : original.replace("[llm]", `[llm]\nproxy = ${JSON.stringify(llmProxy)}`));
        expect(process.env.HTTPS_PROXY).toBe(selected);
      } finally { await fixture.cleanup(); }
      expect(runCli.mock.calls.at(-1)?.[1]).toEqual(["serve", "stop"]);
      await expect(stat(fixture.homePath)).rejects.toMatchObject({ code: "ENOENT" });
    }
  });
});

it("runs an ordinary Native task through the saved window-test proxy, not the connection environment", async () => {
  const proxy = await spawnScriptedOpenAiStreamServer([{ parts: [sseDelta("configured proxy response"), sseFinish("stop"), sseDone()] }]);
  const executable = await ensureTomcatBinary();
  let fixture: Awaited<ReturnType<typeof setupServeFixture>> | undefined;
  let messenger: TomcatMessenger | undefined;
  try {
    const llmProxy = windowTestProxy({ TOMCAT_E2E_WINDOW_PROXY_ENV: "HTTPS_PROXY", HTTPS_PROXY: proxy.baseUrl });
    fixture = await setupServeFixture("http://window-proxy-fixture.invalid", "openai", { binary: executable, llmProxy });
    const saved = await readFile(path.join(fixture.homePath, ".tomcat", "tomcat.config.toml"), "utf8");
    expect(saved).toContain(`proxy = ${JSON.stringify(proxy.baseUrl)}`);
    messenger = new TomcatMessenger({ executable, cwd: fixture.workspacePath, env: {
      ...fixture.env, HTTP_PROXY: "http://native.invalid:bad-port", HTTPS_PROXY: "http://native.invalid:bad-port",
      ALL_PROXY: "http://native.invalid:bad-port", NO_PROXY: "*", no_proxy: "*",
    } });
    await initializeServe(messenger);
    const created = await messenger.request({ type: "new_session", params: { cwd: fixture.workspacePath } });
    expect(created.success).toBe(true);
    const sessionId = (created.payload as { sessionId: string }).sessionId;
    const idle = waitForEvent(messenger, event => event.type === "agent_idle" && event.sessionId === sessionId);
    const [sent, events] = await Promise.all([
      messenger.request({ type: "prompt", sessionId, text: "window-test-proxy-request" }), idle,
    ]);
    expect(sent.success).toBe(true);
    expect(JSON.stringify(events)).toContain("configured proxy response");
    const requests = proxy.capturedNonTitleRequests();
    expect(requests).toHaveLength(1);
    expect(requests[0]).toContain("POST http://window-proxy-fixture.invalid/");
    expect(requests[0]).toContain("window-test-proxy-request");
  } finally {
    try { await messenger?.disposeAsync(); }
    finally { try { await fixture?.cleanup(); } finally { await proxy.close(); } }
  }
}, 120_000);
