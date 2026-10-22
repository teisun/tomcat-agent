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
    expect(env.TOMCAT_HOST_LOCALE).toBe("en");
    expect(env.TOMCAT__UI__LANGUAGE).toBeUndefined();
    expect(env.TOMCAT_AGENT_ACTIVE).toBeUndefined();
    expect({ ...parent, ...env }.ANTHROPIC_API_KEY).toBeUndefined();
    expect(serveFixtureEnvironment("/fixture", {}).TOMCAT_AGENT_ACTIVE).toBeUndefined();
  });

  it("starts cold without init, persisted models, keys, or model overrides", async () => {
    const runCli = vi.fn(async () => undefined);
    const parent = { TOMCAT__LLM__DEFAULT_MODEL: "live-model", OPENAI_API_KEY: "private" };
    const env = serveFixtureEnvironment("/fixture", parent, true);
    expect(parent.OPENAI_API_KEY).toBe("private");
    expect(env.OPENAI_API_KEY).toBeUndefined();
    expect(env.TOMCAT__LLM__DEFAULT_MODEL).toBeUndefined();
    expect(env.TOMCAT__CONTEXT__COMPACTION_MODEL).toBeUndefined();
    const fixture = await setupServeFixture("http://127.0.0.1:1234", "openai", {
      binary: "not-executed", coldStart: true, runCli,
    });
    try {
      expect(runCli).not.toHaveBeenCalled();
      expect(fixture.env.OPENAI_API_KEY).toBeUndefined();
      await expect(stat(path.join(fixture.homePath, ".tomcat"))).rejects.toMatchObject({ code: "ENOENT" });
    } finally { await fixture.cleanup(); }
    expect(runCli).toHaveBeenCalledExactlyOnceWith("not-executed", ["serve", "stop"], expect.objectContaining({ env: fixture.env, cwd: fixture.workspacePath }));
  });

  it("uses the same private home/cwd for initialization, configuration and shutdown", async () => {
    const commands: string[][] = [];
    const fixture = await setupServeFixture("http://127.0.0.1:1234", "openai", {
      binary: "not-executed",
      runCli: async (binary, args, { env, cwd }) => {
        expect(binary).toBe("not-executed");
        expect(cwd).toBe(path.join(env.HOME!, "workspace"));
        commands.push(args);
        if (args[0] === "init") await mkdir(path.join(env.HOME!, ".tomcat"));
        if (args[0] === "serve") expect((await stat(env.HOME!)).isDirectory()).toBe(true);
      },
    });
    const home = fixture.homePath;
    try {
      expect(fixture.env.HOME).toBe(home);
      expect(await readFile(path.join(home, ".tomcat", "models.toml"), "utf8"))
        .toContain('base_url = "http://127.0.0.1:1234"');
      expect(JSON.parse(await readFile(path.join(home, ".tomcat", "mcp.json"), "utf8")))
        .toEqual({ mcpServers: {} });
    } finally {
      const closing = fixture.cleanup();
      expect(fixture.cleanup()).toBe(closing);
      await closing;
    }
    await fixture.cleanup();
    expect(commands).toEqual([
      ["init"], ["model", "key", "set", "openai", "dummy-key"],
      ["model", "default", "gpt-5.4"], ["serve", "stop"],
    ]);
    await expect(stat(home)).rejects.toMatchObject({ code: "ENOENT" });
  });

  it("cleans a partially initialized home and preserves the original error", async () => {
    const firstError = new Error("init failed");
    let home = "";
    await expect(setupServeFixture("http://127.0.0.1:1234", "openai", {
      binary: "not-executed",
      runCli: async (_binary, args, { env }) => {
        home = env.HOME!;
        if (args[0] === "init") {
          await mkdir(path.join(home, ".tomcat"));
          throw firstError;
        }
        expect(args).toEqual(["serve", "stop"]);
      },
    })).rejects.toBe(firstError);
    expect(home).not.toBe("");
    await expect(stat(home)).rejects.toMatchObject({ code: "ENOENT" });
  });

  it("also cleans if model setup fails after initialization", async () => {
    const failure = new Error("model setup failed");
    let home = "";
    await expect(setupServeFixture("http://127.0.0.1:1234", "openai", {
      binary: "not-executed",
      runCli: async (_binary, args, { env }) => {
        home = env.HOME!;
        if (args[0] === "init") await mkdir(path.join(home, ".tomcat"));
        if (args[0] === "model") throw failure;
      },
    })).rejects.toBe(failure);
    await expect(stat(home)).rejects.toMatchObject({ code: "ENOENT" });
  });

  it("retains the private home when shutdown fails and allows explicit cleanup retry", async () => {
    const failure = new Error("service has not stopped");
    const runCli = vi.fn().mockRejectedValueOnce(failure).mockResolvedValue(undefined);
    const fixture = await setupServeFixture("http://127.0.0.1:1234", "openai", {
      binary: "not-executed", coldStart: true, runCli,
    });
    try {
      await expect(fixture.cleanup()).rejects.toBe(failure);
      expect((await stat(fixture.homePath)).isDirectory()).toBe(true);
    } finally { await fixture.cleanup(); }
    expect(runCli).toHaveBeenCalledTimes(2);
    await expect(stat(fixture.homePath)).rejects.toMatchObject({ code: "ENOENT" });
  });

  it.each(["/v1/chat/completions", "/v1/responses"])("does not consume agent responses for session/turn titles at %s", async (endpoint) => {
    const server = await spawnScriptedOpenAiStreamServer([{ parts: [sseDone()] }]);
    const prompts = ["Generate a short chat title from the user's first message.", "Generate a short title summarizing this AI assistant thinking block and its tool usage."];
    try {
      for (const [prompt, stream] of prompts.flatMap(prompt => [[prompt, false], [prompt, true]] as const)) {
        const response = await fetch(`${server.baseUrl}${endpoint}`, { method: "POST", body: JSON.stringify({ model: "utility", stream, messages: [{ role: "user", content: prompt }] }) });
        expect(response.status).toBe(200);
        const body = await response.json();
        expect(endpoint === "/v1/responses" ? body.output[0].content[0].text : body.choices[0].message.content).toBe("Generated title");
      }
      // The same text in an ordinary agent turn (which always carries tools) must not be mistaken for a utility call.
      const agent = await fetch(`${server.baseUrl}${endpoint}`, { method: "POST", body: JSON.stringify({ stream: true, tools: [{ type: "function", name: "read" }], messages: [{ role: "user", content: prompts[1] }] }) });
      expect(await agent.text()).toBe(sseDone().body);
      expect(server.capturedRequests()).toHaveLength(5);
      expect(server.capturedNonTitleRequests()).toHaveLength(1);
      const { utilityRequests } = await (await fetch(`${server.baseUrl}/_test/request-count`)).json();
      expect(utilityRequests).toEqual([false, false, true, true].map(turnSummary => ({ model: "utility", turnSummary })));
    } finally { await server.close(); }
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
      expect(await (await fetch(`${server.baseUrl}/_test/request-count`)).json()).toEqual({ count: 1, modelListAuthorizations: [], utilityRequests: [] });
      expect(server.capturedRequests()).toHaveLength(1);
      const started = Date.now();
      await server.close();
      expect(Date.now() - started).toBeLessThan(2_000);
    } finally {
      request.destroy();
      await server.close();
    }
  });
});
