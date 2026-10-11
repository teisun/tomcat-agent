import * as fs from "node:fs/promises";
import * as path from "node:path";
import { expect, it } from "vitest";
import { createHostE2eFixture } from "../scripts/e2eHostFixture";
import { TEST_HISTORY_PAGE_GATE_FILE } from "../src/constants";
import { initializeServe } from "../src/serveClient/initialize";
import { TomcatMessenger } from "../src/serveClient/TomcatMessenger";

it("holds only older history pages until the Host has observed loading", async () => {
  const fixture = await createHostE2eFixture();
  const gate = path.join(fixture.workspaceDir, TEST_HISTORY_PAGE_GATE_FILE);
  const messenger = new TomcatMessenger({
    executable: fixture.fakeServePath,
    env: { ...fixture.env, TOMCAT_E2E_HISTORY_PAGE_DELAY_MS: "0" },
  });
  try {
    const initialized = await initializeServe(messenger);
    const sessionId = initialized.sessionId!;
    await fs.writeFile(gate, "held", { flag: "wx" });
    // Initial hydration must remain available while the older-page response is held.
    expect((await messenger.sendGetMessages(sessionId)).success).toBe(true);
    let received = false;
    const pending = messenger.sendGetMessages(sessionId, { cursor: "1", limit: 1 }, 2_000).then(
      value => { received = true; return { value }; },
      error => { received = true; return { error }; },
    );
    // Ordered roundtrip, not a sleep: the fake backend has processed the page request.
    expect((await messenger.request({ type: "get_state", sessionId })).success).toBe(true);
    expect(received).toBe(false);
    await fs.unlink(gate);
    const result = await pending;
    if ("error" in result) throw result.error;
    expect(result.value.success).toBe(true);
  } finally {
    await messenger.disposeAsync();
    await fixture.cleanup();
  }
});
