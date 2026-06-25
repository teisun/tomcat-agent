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
          thinkingLevel?: string | null;
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

    const initialState = await waitForWebviewState(
      api,
      (state) => {
        const sessionId = state.activeSessionId;
        if (!sessionId) {
          return undefined;
        }
        return state.sessionViews[sessionId]?.model ? state : undefined;
      },
      10_000,
    );
    const sessionId = initialState.activeSessionId;
    assert.ok(sessionId, "expected an active webview session");

    await api.__testing.sendWebviewIntent({
      data: {
        level: "xhigh",
        modelId: "gpt-5.4",
        sessionId,
      },
      messageId: "model-switch-gpt-effort-xhigh",
      type: "setThinkingLevel",
    });
    const gptRaised = await waitForWebviewState(
      api,
      (state) => {
        const activeId = state.activeSessionId;
        if (!activeId) {
          return undefined;
        }
        const session = state.sessionViews[activeId];
        return session?.model === "gpt-5.4" && session.thinkingLevel === "xhigh"
          ? state
          : undefined;
      },
      10_000,
    );
    screenshots.push(await captureScreenshot("02-gpt-effort-xhigh.png"));

    await api.__testing.sendWebviewIntent({
      data: {
        modelId: "deepseek-v4-pro",
        sessionId,
      },
      messageId: "model-switch-to-deepseek",
      type: "setModel",
    });
    const deepseekState = await waitForWebviewState(
      api,
      (state) => {
        const activeId = state.activeSessionId;
        if (!activeId) {
          return undefined;
        }
        return state.sessionViews[activeId]?.model === "deepseek-v4-pro"
          ? state
          : undefined;
      },
      10_000,
    );
    await api.__testing.sendWebviewIntent({
      data: {
        level: "medium",
        modelId: "deepseek-v4-pro",
        sessionId,
      },
      messageId: "model-switch-deepseek-effort-medium",
      type: "setThinkingLevel",
    });
    const deepseekRaised = await waitForWebviewState(
      api,
      (state) => {
        const activeId = state.activeSessionId;
        if (!activeId) {
          return undefined;
        }
        const current = state.sessionViews[activeId];
        return current?.model === "deepseek-v4-pro" && current.thinkingLevel === "medium"
          ? state
          : undefined;
      },
      10_000,
    );
    screenshots.push(await captureScreenshot("03-deepseek-effort-medium.png"));

    await api.__testing.sendWebviewIntent({
      data: {
        modelId: "gpt-5.4",
        sessionId,
      },
      messageId: "model-switch-back-to-gpt",
      type: "setModel",
    });
    const gptRestored = await waitForWebviewState(
      api,
      (state) => {
        const activeId = state.activeSessionId;
        if (!activeId) {
          return undefined;
        }
        const current = state.sessionViews[activeId];
        return current?.model === "gpt-5.4" && current.thinkingLevel === "xhigh"
          ? state
          : undefined;
      },
      10_000,
    );
    screenshots.push(await captureScreenshot("04-gpt-effort-restored.png"));

    await api.__testing.restartServe();
    await api.__testing.waitForWebviewReady(20_000);
    await pause(500);
    const restartRecovered = await waitForWebviewState(
      api,
      (state) => {
        const activeId = state.activeSessionId;
        if (!activeId) {
          return undefined;
        }
        const current = state.sessionViews[activeId];
        return current?.model === "gpt-5.4" && current.thinkingLevel === "xhigh"
          ? state
          : undefined;
      },
      15_000,
    );
    screenshots.push(await captureScreenshot("05-after-restart-gpt-effort.png"));

    await api.__testing.sendWebviewIntent({
      data: {
        modelId: "deepseek-v4-pro",
        sessionId: restartRecovered.activeSessionId,
      },
      messageId: "model-switch-after-restart-to-deepseek",
      type: "setModel",
    });
    const restartDeepseek = await waitForWebviewState(
      api,
      (state) => {
        const activeId = state.activeSessionId;
        if (!activeId) {
          return undefined;
        }
        const current = state.sessionViews[activeId];
        return current?.model === "deepseek-v4-pro" && current.thinkingLevel === "medium"
          ? state
          : undefined;
      },
      10_000,
    );
    screenshots.push(await captureScreenshot("06-after-restart-deepseek-effort.png"));

    const report = {
      artifactsRoot: path.dirname(reportPath),
      checks: {
        closeButtonRemoved: {
          passed: closeButtonAbsent,
        },
        deepseekEffort: {
          passed:
            deepseekRaised.sessionViews[deepseekRaised.activeSessionId ?? ""]?.model ===
              "deepseek-v4-pro" &&
            deepseekRaised.sessionViews[deepseekRaised.activeSessionId ?? ""]?.thinkingLevel ===
              "medium",
        },
        gptEffort: {
          passed:
            gptRaised.sessionViews[gptRaised.activeSessionId ?? ""]?.model === "gpt-5.4" &&
            gptRaised.sessionViews[gptRaised.activeSessionId ?? ""]?.thinkingLevel === "xhigh",
        },
        perModelRestore: {
          passed:
            gptRestored.sessionViews[gptRestored.activeSessionId ?? ""]?.thinkingLevel ===
              "xhigh" &&
            deepseekState.sessionViews[deepseekState.activeSessionId ?? ""]?.model ===
              "deepseek-v4-pro",
        },
        restartPersistence: {
          passed:
            restartRecovered.sessionViews[restartRecovered.activeSessionId ?? ""]?.thinkingLevel ===
              "xhigh" &&
            restartDeepseek.sessionViews[restartDeepseek.activeSessionId ?? ""]?.thinkingLevel ===
              "medium",
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
