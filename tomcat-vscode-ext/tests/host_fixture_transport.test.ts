import * as path from "node:path";
import { expect, it } from "vitest";
import { createHostE2eFixture } from "../scripts/e2eHostFixture";
import { runGateCommand } from "../scripts/run-vscode-full-gate";
import { initializeServe } from "../src/serveClient/initialize";
import { TomcatMessenger } from "../src/serveClient/TomcatMessenger";
import { connectSharedService, runSharedServiceAction } from "../src/serveClient/sharedConnection";

it("runs the existing host fixture through production CLI discovery and native IPC", async () => {
  const fixture = await createHostE2eFixture();
  const options = {
    executable: process.execPath, extraArgs: [fixture.fakeServePath], env: fixture.env,
    cwd: fixture.workspaceDir, signal: new AbortController().signal,
    allowStart: true, onStderr: () => {},
  };
  // No connectFactory override: the old fixture failed before reaching initialize.
  const messenger = new TomcatMessenger(options);
  let connection: Awaited<ReturnType<typeof connectSharedService>> | undefined;
  try {
    connection = await connectSharedService(options);
    const instanceId = connection.info.instanceId;
    const initialized = await initializeServe(messenger);
    expect(initialized.sessionId).toBeTruthy();
    expect((await messenger.request({ type: "get_state", sessionId: initialized.sessionId! })).success).toBe(true);
    await runSharedServiceAction(options, "stop");
    await expect(connectSharedService({ ...options, allowStart: false })).rejects.toThrow("unavailable");
    await runSharedServiceAction(options, "restart");
    connection.socket.destroy();
    connection = await connectSharedService(options);
    expect(connection.info.instanceId).not.toBe(instanceId);
  } finally {
    connection?.socket.destroy();
    await messenger.disposeAsync();
    await Promise.all([fixture.cleanup(), fixture.cleanup()]);
  }
}, 15_000);

it("keeps the fixture IPC responsive while running a gate command", async () => {
  const fixture = await createHostE2eFixture();
  try {
    // The child bounds the failure even if a synchronous runner blocks Vitest's timers.
    const probe = `
      process.argv = [process.execPath, process.argv[1], "serve", "status"];
      setTimeout(() => { console.error("fixture IPC blocked by the gate runner"); process.exit(2); }, 2000);
      require(process.argv[1]);
    `;
    await expect(Promise.resolve().then(() => runGateCommand(
      process.execPath, ["-e", probe, fixture.fakeServePath], fixture.workspaceDir, fixture.env,
    ))).resolves.toBeUndefined();
  } finally {
    await fixture.cleanup();
  }
}, 10_000);

it("reports both child exit failures and launch errors to the gate", async () => {
  const fixture = await createHostE2eFixture();
  try {
    await expect(runGateCommand(
      process.execPath, ["-e", "process.exit(7)"], fixture.workspaceDir,
    )).rejects.toThrow("exited with 7");
    await expect(runGateCommand(
      path.join(fixture.workspaceDir, "missing-command"), [], fixture.workspaceDir,
    )).rejects.toMatchObject({ code: "ENOENT" });
  } finally {
    await fixture.cleanup();
  }
});
