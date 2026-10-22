import { expect, it } from "vitest";
import { createHostE2eFixture } from "../scripts/e2eHostFixture";
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
