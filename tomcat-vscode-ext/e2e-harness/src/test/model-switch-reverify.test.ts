import * as assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import * as fs from "node:fs/promises";
import * as path from "node:path";

const repoRoot = path.resolve(__dirname, "../../../");
const hostE2e = require(path.resolve(
  repoRoot,
  "out/test/suite/support/hostE2eScenario.js",
)) as {
  getTomcatExtensionApi(): Promise<TomcatApi>;
};

type DomSnapshot = Awaited<ReturnType<TomcatApi["__testing"]["captureWebviewDom"]>>;
type WebviewState = ReturnType<TomcatApi["__testing"]["getWebviewState"]>;

type TomcatApi = {
  __testing: {
    captureWebviewDom(): Promise<{
      activeSessionId: string | null;
      html: string;
      messageTexts: string[];
    }>;
    clearObservedEvents(): void;
    focusWebview(): Promise<void>;
    getWebviewState(): {
      activeSessionId: string | null;
      sessionViews: Record<
        string,
        {
          model?: string | null;
          timeline: Array<{
            kind?: string;
            text?: string;
            type: string;
          }>;
        }
      >;
    };
    restartServe(): Promise<void>;
    sendWebviewIntent(intent: {
      data?: Record<string, unknown>;
      messageId: string;
      type: string;
    }): Promise<void>;
    waitForEvent(filter: {
      textIncludes?: string;
      timeoutMs?: number;
      type?: string;
    }): Promise<unknown>;
    waitForWebviewReady(timeoutMs?: number): Promise<void>;
  };
};

function requireEnv(name: string): string {
  const value = process.env[name];
  assert.ok(value, `expected ${name} to be defined`);
  return value;
}

async function pause(ms: number): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, ms));
}

async function captureScreenshot(name: string): Promise<string> {
  const screenshotsDir = requireEnv("TOMCAT_ACCEPT_SCREENSHOTS_DIR");
  const targetPath = path.join(screenshotsDir, name);
  await pause(300);
  execFileSync("screencapture", ["-x", targetPath], {
    stdio: "inherit",
  });
  return targetPath;
}

async function waitForDom<T>(
  api: TomcatApi,
  predicate: (snapshot: DomSnapshot) => T | undefined,
  timeoutMs = 15_000,
): Promise<T> {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    const snapshot = await api.__testing.captureWebviewDom();
    const result = predicate(snapshot);
    if (result !== undefined) {
      return result;
    }
    await pause(100);
  }
  throw new Error("Timed out waiting for webview DOM to match the expected condition");
}

async function waitForWebviewState<T>(
  api: TomcatApi,
  predicate: (state: WebviewState) => T | undefined,
  timeoutMs = 15_000,
): Promise<T> {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    const state = api.__testing.getWebviewState();
    const result = predicate(state);
    if (result !== undefined) {
      return result;
    }
    await pause(100);
  }
  throw new Error("Timed out waiting for webview state to match the expected condition");
}

suite("Tomcat model switch reverify", () => {
  test("reverifies webview model switching in a real VSCode host", async () => {
    const screenshots: string[] = [];
    const reportPath = requireEnv("TOMCAT_ACCEPT_REPORT_PATH");
    const api = await hostE2e.getTomcatExtensionApi();

    await api.__testing.focusWebview();
    await api.__testing.waitForWebviewReady(20_000);

    const hydrated = await waitForDom(
      api,
      (snapshot) =>
        snapshot.activeSessionId &&
        snapshot.messageTexts.some((text) => /Historic prompt 6/i.test(text))
          ? snapshot
          : undefined,
      20_000,
    );
    screenshots.push(await captureScreenshot("01-model-switch-initial.png"));

    const closeButtonAbsent =
      !hydrated.html.includes("Close active session") && !hydrated.html.includes(">Close<");

    const initialState = api.__testing.getWebviewState();
    assert.ok(initialState.activeSessionId, "expected an active webview session");

    await api.__testing.sendWebviewIntent({
      data: {
        modelId: "deepseek-v4-pro",
        sessionId: initialState.activeSessionId,
      },
      messageId: "model-switch-to-deepseek",
      type: "setModel",
    });
    const deepseekState = await waitForWebviewState(
      api,
      (state) => {
        const sessionId = state.activeSessionId;
        if (!sessionId) {
          return undefined;
        }
        return state.sessionViews[sessionId]?.model === "deepseek-v4-pro"
          ? state
          : undefined;
      },
      10_000,
    );
    screenshots.push(await captureScreenshot("02-model-switched-deepseek.png"));

    api.__testing.clearObservedEvents();
    await api.__testing.sendWebviewIntent({
      data: {
        sessionId: deepseekState.activeSessionId,
        text: "trigger capability mismatch",
      },
      messageId: "model-switch-capability-mismatch",
      type: "prompt",
    });
    await api.__testing.waitForEvent({
      timeoutMs: 15_000,
      type: "agent_end",
    });
    const mismatchState = await waitForWebviewState(
      api,
      (state) => {
        const sessionId = state.activeSessionId;
        if (!sessionId) {
          return undefined;
        }
        const session = state.sessionViews[sessionId];
        if (!session) {
          return undefined;
        }
        return session.timeline.some(
          (item) =>
            item.type === "message" &&
            item.kind === "error" &&
            typeof item.text === "string" &&
            /provider\/model 不支持 vision/i.test(item.text),
        )
          ? state
          : undefined;
      },
      10_000,
    );
    screenshots.push(await captureScreenshot("03-capability-mismatch-visible.png"));

    await api.__testing.restartServe();
    await api.__testing.waitForWebviewReady(20_000);
    await pause(500);
    api.__testing.clearObservedEvents();
    await api.__testing.sendWebviewIntent({
      data: {
        text: "after restart prompt",
      },
      messageId: "model-switch-after-restart",
      type: "prompt",
    });
    await api.__testing.waitForEvent({
      textIncludes: "Manual acceptance reply for prompt: after restart prompt",
      timeoutMs: 15_000,
      type: "message_update",
    });
    const restartRecovered = await waitForDom(
      api,
      (snapshot) =>
        snapshot.messageTexts.some((text) => /after restart prompt/i.test(text))
          ? snapshot
          : undefined,
      10_000,
    );
    screenshots.push(await captureScreenshot("04-after-restart-recovered.png"));

    const report = {
      artifactsRoot: path.dirname(reportPath),
      checks: {
        capabilityMismatch: {
          passed: mismatchState.sessionViews[mismatchState.activeSessionId ?? ""]?.timeline.some(
            (item) =>
              item.type === "message" &&
              item.kind === "error" &&
              typeof item.text === "string" &&
              /provider\/model 不支持 vision/i.test(item.text),
          ) === true,
        },
        closeButtonRemoved: {
          passed: closeButtonAbsent,
        },
        modelSwitch: {
          passed:
            deepseekState.sessionViews[deepseekState.activeSessionId ?? ""]?.model ===
            "deepseek-v4-pro",
        },
        restart: {
          passed: restartRecovered.messageTexts.some((text) => /after restart prompt/i.test(text)),
        },
      },
      screenshots,
    };

    await fs.writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`, "utf8");

    for (const [name, value] of Object.entries(report.checks)) {
      assert.equal(value.passed, true, `expected ${name} reverify check to pass`);
    }
  });
});
