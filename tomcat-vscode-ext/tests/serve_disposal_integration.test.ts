import { spawn } from "node:child_process";
import { expect, it } from "vitest";
import { TomcatMessenger } from "../src/serveClient/TomcatMessenger";
import { ensureTomcatBinary, setupServeFixture } from "./serveTestUtils";

it("closes a real offline serve and every stdio channel before fixture removal", async () => {
  const fixture = await setupServeFixture("http://127.0.0.1:1");
  const events: string[] = [];
  const messenger = new TomcatMessenger({
    executable: await ensureTomcatBinary(), cwd: fixture.workspacePath, env: fixture.env,
    spawnFactory: ((exe, args, options) => {
      const child = spawn(exe, args, options ?? {});
      for (const name of ["exit", "close", "error"]) child.on(name, (...values) => events.push(`${name}: ${values}`));
      for (const name of ["stdin", "stdout", "stderr"] as const) child[name]?.on("close", () => events.push(`${name}: close`));
      return child;
    }) as typeof spawn,
  });
  try {
    await messenger.request({ type: "get_state" });
    await messenger.disposeAsync();
    expect(events, events.join("; ")).toContain("stdout: close");
    expect(events, events.join("; ")).toContain("stderr: close");
  } finally {
    await messenger.disposeAsync();
    await fixture.cleanup();
  }
}, 120_000);
