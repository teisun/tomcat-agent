import * as http from "node:http";
import { mkdir, stat, readFile } from "node:fs/promises";
import { connect } from "node:net";
import * as path from "node:path";
import { describe, expect, it, vi } from "vitest";
import { serveFixtureEnvironment, setupServeFixture, spawnScriptedOpenAiStreamServer, sseDone, sseFinish } from "./serveTestUtils";

describe("serve fixture resource ownership", () => {
  it("emits a valid JSON finish frame for scripted Chat Completions", () => {
    for (const reason of ["stop", "tool_calls"]) {
      expect(JSON.parse(sseFinish(reason).body.slice("data: ".length)))
        .toEqual({ choices: [{ finish_reason: reason }] });
    }
  });

  it("isolates config, models, credentials, and the disposable child process", () => {
    const parent = {
      HOME: "/real-home", PATH: "/bin", TOMCAT_AGENT_ACTIVE: "1",
      TOMCAT__AGENT__ID: "other", TOMCAT__STORAGE__WORK_DIR: "/real-storage",
      TOMCAT__LLM__DEFAULT_MODEL: "live-model", ANTHROPIC_API_KEY: "not-a-real-key",
      HTTP_PROXY: "http://proxy", https_proxy: "http://proxy",
    };
    const before = { ...parent };
    const env = serveFixtureEnvironment("/private-fixture", parent);
    expect(parent).toEqual(before);
    expect(env).toMatchObject({ HOME: "/private-fixture", USERPROFILE: "/private-fixture",
      PATH: "/bin", OPENAI_API_KEY: "dummy-key",
      TOMCAT__STORAGE__WORK_DIR: path.join("/private-fixture", ".tomcat"),
      TOMCAT__LLM__DEFAULT_MODEL: "gpt-5.4", HTTP_PROXY: "", https_proxy: "" });
    expect(env.ANTHROPIC_API_KEY).toBeUndefined();
    expect(env.TOMCAT__AGENT__ID).toBeUndefined();
    expect(env.TOMCAT_AGENT_ACTIVE).toBeUndefined();
    expect({ ...parent, ...env }.ANTHROPIC_API_KEY).toBeUndefined();
    expect(serveFixtureEnvironment("/fixture", {}).TOMCAT_AGENT_ACTIVE).toBeUndefined();
  });

  it("uses the same private home/cwd for initialization and runtime", async () => {
    const fixture = await setupServeFixture("http://127.0.0.1:1234", "openai", {
      binary: "not-executed",
      initialize: async (binary, env, cwd) => {
        expect(binary).toBe("not-executed");
        expect(cwd).toBe(path.join(env.HOME!, "workspace"));
        await mkdir(path.join(env.HOME!, ".tomcat"));
      },
    });
    const home = fixture.homePath;
    try {
      expect(fixture.env.HOME).toBe(home);
      expect(await readFile(path.join(home, ".tomcat", "models.toml"), "utf8"))
        .toContain('base_url = "http://127.0.0.1:1234"');
      expect(JSON.parse(await readFile(path.join(home, ".tomcat", "mcp.json"), "utf8")))
        .toEqual({ mcpServers: {} });
    } finally { await fixture.cleanup(); }
    await fixture.cleanup();
    await expect(stat(home)).rejects.toMatchObject({ code: "ENOENT" });
  });

  it("cleans a partially initialized home and preserves the original error", async () => {
    const firstError = new Error("init failed");
    let home = "";
    await expect(setupServeFixture("http://127.0.0.1:1234", "openai", {
      binary: "not-executed",
      initialize: async (_binary, env) => {
        home = env.HOME!;
        await mkdir(path.join(home, ".tomcat"));
        throw firstError;
      },
    })).rejects.toBe(firstError);
    expect(home).not.toBe("");
    await expect(stat(home)).rejects.toMatchObject({ code: "ENOENT" });
  });

  it("also cleans if model setup fails after initialization", async () => {
    let home = "";
    await expect(setupServeFixture("http://127.0.0.1:1234", "openai", {
      binary: "not-executed",
      initialize: async (_binary, env) => { home = env.HOME!; },
    })).rejects.toMatchObject({ code: "ENOENT" });
    await expect(stat(home)).rejects.toMatchObject({ code: "ENOENT" });
  });

  it("closes a socket with an unfinished request and allows repeated close", async () => {
    const server = await spawnScriptedOpenAiStreamServer([]);
    const url = new URL(server.baseUrl);
    const socket = connect(Number(url.port), url.hostname);
    socket.on("error", () => {});
    const socketClosed = new Promise<void>((resolve) => socket.once("close", () => resolve()));
    await new Promise<void>((resolve, reject) => {
      socket.once("connect", resolve);
      socket.once("error", reject);
    });
    socket.write("POST /v1/chat/completions HTTP/1.1\r\nHost: localhost\r\n");
    const closing = server.close();
    expect(server.close()).toBe(closing);
    await closing;
    await socketClosed;
  });

  it("cancels delayed responses rather than waiting for the scripted delay", async () => {
    const server = await spawnScriptedOpenAiStreamServer([
      { parts: [{ ...sseDone(), delayMs: 60_000 }] },
    ]);
    const request = http.request(`${server.baseUrl}/v1/chat/completions`, { method: "POST" });
    request.on("error", () => {});
    request.end(JSON.stringify({ stream: true, messages: [] }));
    try {
      await vi.waitFor(() => expect(server.capturedRequests()).toHaveLength(1));
      const started = Date.now();
      await server.close();
      expect(Date.now() - started).toBeLessThan(2_000);
    } finally {
      request.destroy();
      await server.close();
    }
  });
});
