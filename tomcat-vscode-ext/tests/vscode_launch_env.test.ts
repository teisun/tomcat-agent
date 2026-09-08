import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import * as vm from "node:vm";
import { Writable } from "node:stream";
import ts from "typescript";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanVsCodeGuiEnvironment, GUI_MODE_KEYS, runVsCodeGuiTests, withVsCodeGuiEnvironment } from "../scripts/vscodeLaunchEnv";

const owned: string[] = [];
async function temp() {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "gui-launch-test-"));
  owned.push(dir);
  return dir;
}
afterEach(async () => {
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
  await Promise.all(owned.splice(0).map((dir) => fs.rm(dir, { recursive: true, force: true })));
});
const silent = () => new Writable({ write(_chunk, _encoding, done) { done(); } });

function assertClean(supplied: NodeJS.ProcessEnv) {
  const merged = { ...process.env, ...supplied }; // actual test-electron merge
  for (const key of GUI_MODE_KEYS) expect(merged).not.toHaveProperty(key);
}

describe("VS Code GUI launch environment", () => {
  it.each([[false, false], [true, false], [false, true], [true, true]])(
    "cleans parent=%s and supplied=%s without changing ordinary settings", async (parent, supplied) => {
      for (const key of GUI_MODE_KEYS) vi.stubEnv(key, parent ? "polluted" : undefined);
      const env: NodeJS.ProcessEnv = { PATH: "/bin", TOMCAT_AGENT_ACTIVE: "1", FIXTURE_VALUE: "kept" };
      if (supplied) for (const key of GUI_MODE_KEYS) env[key] = "supplied-pollution";
      const before = { ...env };
      expect(await withVsCodeGuiEnvironment(env, (clean) => {
        assertClean(clean);
        expect(clean).toMatchObject({ PATH: "/bin", TOMCAT_AGENT_ACTIVE: "1", FIXTURE_VALUE: "kept" });
        return 42;
      })).toBe(42);
      expect(env).toEqual(before);
      for (const key of GUI_MODE_KEYS) expect(process.env[key]).toBe(parent ? "polluted" : undefined);
      expect(cleanVsCodeGuiEnvironment(env)).not.toBe(env);
    },
  );

  it.each([undefined, "", "1"])("restores exact original value %s on sync throw and async reject", async (value) => {
    vi.stubEnv("ELECTRON_RUN_AS_NODE", value);
    const firstError = new Error("original failure");
    await expect(withVsCodeGuiEnvironment({}, () => { throw firstError; })).rejects.toBe(firstError);
    expect(process.env.ELECTRON_RUN_AS_NODE).toBe(value);
    await expect(withVsCodeGuiEnvironment({}, async () => { throw firstError; })).rejects.toBe(firstError);
    expect(process.env.ELECTRON_RUN_AS_NODE).toBe(value);
  });

  it("shares a private capture port with the harness and preserves an existing devhost port", async () => {
    const root = await temp();
    for (const port of [undefined, 12345]) {
      const launchArgs = port ? [`--remote-debugging-port=${port}`] : [];
      const before = [...launchArgs];
      await runVsCodeGuiTests(async (received) => {
        const supplied = Number(received.extensionTestsEnv!.TOMCAT_E2E_CDP_PORT);
        expect(supplied).toBeGreaterThan(0);
        expect(received.launchArgs!.filter((arg) => arg.startsWith("--remote-debugging-port=")))
          .toEqual([`--remote-debugging-port=${supplied}`]);
        if (port) expect(supplied).toBe(port);
        return 0;
      }, { launchArgs, extensionTestsEnv: { TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR: root } });
      expect(launchArgs).toEqual(before);
    }
  });

  it("rejects overlapping scopes without disturbing the active launch", async () => {
    vi.stubEnv("ELECTRON_RUN_AS_NODE", "1");
    let release!: () => void;
    const first = withVsCodeGuiEnvironment({}, () => new Promise<void>((resolve) => { release = resolve; }));
    try {
      await expect(withVsCodeGuiEnvironment({}, () => {})).rejects.toThrow("overlapping");
      expect(process.env.ELECTRON_RUN_AS_NODE).toBeUndefined();
    } finally { release(); await first; }
    expect(process.env.ELECTRON_RUN_AS_NODE).toBe("1");
  });

  it("retains the first failure, redacted host logs and an explicit missing-screenshot record", async () => {
    const root = await temp();
    const userData = path.join(root, "profile");
    await fs.mkdir(path.join(userData, "logs"), { recursive: true });
    const secret = "fake-secret-for-redaction";
    await fs.writeFile(path.join(userData, "logs", "host.log"), `startup ${secret}`);
    const firstError = new Error("window did not start");
    const options = {
      extensionTestsEnv: { TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR: root, TEST_API_KEY: secret },
      launchArgs: [`--user-data-dir=${userData}`], stdout: silent(), stderr: silent(),
    };
    await expect(runVsCodeGuiTests(async (received) => {
      received.stderr!.write(`diagnostic ${secret}`);
      throw firstError;
    }, options)).rejects.toBe(firstError);
    const run = (await fs.readdir(root)).find((entry) => entry.startsWith("gui-"))!;
    const artifacts = path.join(root, run);
    expect(JSON.parse(await fs.readFile(path.join(artifacts, "result.json"), "utf8"))).toMatchObject({ status: "failed" });
    expect(await fs.readFile(path.join(artifacts, "host-logs", "host.log"), "utf8")).toBe("startup [REDACTED]");
    expect(await fs.readFile(path.join(artifacts, "stderr.log"), "utf8")).not.toContain(secret);
    expect(await fs.readFile(path.join(artifacts, "screenshots-not-produced.txt"), "utf8")).toContain("not established");
  });

  it("indexes manual/image PNGs in their own run directory without a false missing marker", async () => {
    const root = await temp();
    const screenshots = path.join(root, "screenshots");
    await fs.mkdir(screenshots);
    await runVsCodeGuiTests(async () => {
      await fs.writeFile(path.join(screenshots, "capture.png"), "fixture PNG");
      return 0;
    }, { extensionTestsEnv: { TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR: root, TOMCAT_ACCEPT_SCREENSHOTS_DIR: screenshots } });
    const run = (await fs.readdir(root)).find((entry) => entry.startsWith("gui-"))!;
    expect(await fs.readFile(path.join(root, run, "screenshots.json"), "utf8")).toContain("capture.png");
    await expect(fs.stat(path.join(root, run, "screenshots-not-produced.txt"))).rejects.toMatchObject({ code: "ENOENT" });
  });

  it("uses new artifact directories and treats nonzero runner results as failures", async () => {
    const root = await temp();
    const options = { extensionTestsEnv: { TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR: root }, stdout: silent(), stderr: silent() };
    await runVsCodeGuiTests(async () => 0, options);
    await expect(runVsCodeGuiTests(async () => 3, options)).rejects.toThrow("exited with 3");
    expect((await fs.readdir(root)).filter((entry) => entry.startsWith("gui-"))).toHaveLength(2);
  });

  it("does not replace a launch failure with a later artifact write failure", async () => {
    const root = await temp();
    const first = new Error("first launch error");
    const warning = vi.spyOn(console, "warn").mockImplementation(() => {});
    await expect(runVsCodeGuiTests(async (options) => {
      await fs.mkdir(path.join(options.extensionTestsEnv!.TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR!, "result.json"));
      throw first;
    }, { extensionTestsEnv: { TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR: root } })).rejects.toBe(first);
    expect(warning).toHaveBeenCalledOnce();
  });

  it.each(["verify-vsix", "install-e2e", "devhost", "manual-acceptance", "image-acceptance"])(
    "executes the actual %s launch call with a stub runner (without running main/install/build)", async (name) => {
      const root = await temp();
      const file = path.resolve(__dirname, `../scripts/run-vscode-${name}.ts`);
      const source = ts.createSourceFile(file, await fs.readFile(file, "utf8"), ts.ScriptTarget.Latest, true);
      const calls: ts.CallExpression[] = [];
      const visit = (node: ts.Node) => {
        if (ts.isCallExpression(node) && node.expression.getText(source) === "runVsCodeGuiTests") calls.push(node);
        ts.forEachChild(node, visit);
      };
      visit(source);
      expect(calls).toHaveLength(1);
      const env = { TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR: root, ELECTRON_RUN_AS_NODE: "1", TOMCAT_AGENT_ACTIVE: "1" };
      const runTests = vi.fn(async (options) => { assertClean(options.extensionTestsEnv); return 0; });
      // Evaluate the real launch expression, not a duplicate of its options.
      const locals = {
        path, process: { env }, runVsCodeGuiTests, runTests,
        resolveVsCodeExecutable: () => "fake-vscode",
        options: { harnessRoot: root, harnessTestsPath: "fake-test", testEnv: env, workspacePath: root, artifactsDir: root },
        fixture: { env }, extensionTestsEnv: env, harnessRoot: root,
        extensionDevelopmentPath: root, extensionTestsPath: "fake-test", harnessTestsPath: "fake-test",
        extensionRoot: root, userDataDir: path.join(root, "profile"), extensionsDir: root,
        workspaceDir: root, screenshotsDir: root, reportPath: path.join(root, "report.json"),
        artifactsRoot: root, fakeServeStateDir: root, cdpPort: 12345,
      };
      const code = ts.transpileModule(`${calls[0].getText(source)};`, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
      await vm.runInNewContext(code, locals);
      expect(runTests).toHaveBeenCalledOnce();
      expect(runTests.mock.calls[0][0].extensionTestsEnv.TOMCAT_AGENT_ACTIVE).toBe("1");
    },
  );
});
