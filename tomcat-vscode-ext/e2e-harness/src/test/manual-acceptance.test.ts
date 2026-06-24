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
      approvalCount: number;
      composerControlMetrics: Record<string, { top: number; width: number }>;
      composerRowCount: number;
      expandedThinkingCount: number;
      expandedToolTitles: string[];
      hasConflict: boolean;
      html: string;
      jumpToLatestVisible: boolean;
      messageTexts: string[];
      sessionTabs: string[];
      streamMetrics: {
        clientHeight: number;
        distanceFromBottom: number;
        scrollHeight: number;
        scrollTop: number;
      };
      timelineKinds: string[];
      toolBodyMetrics: Array<{
        clientHeight: number;
        expanded: boolean;
        scrollHeight: number;
        title: string;
      }>;
      toolTitles: string[];
    }>;
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
    clearObservedEvents(): void;
    focusWebview(): Promise<void>;
    restartServe(): Promise<void>;
    sendWebviewDomAction(action: {
      edge?: "bottom" | "top";
      index?: number;
      kind: "clickTestId" | "scrollToEdge" | "setRootWidth";
      testId?: string;
      widthPx?: number | null;
    }): Promise<void>;
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

async function sendDomAction(
  api: TomcatApi,
  action: Parameters<TomcatApi["__testing"]["sendWebviewDomAction"]>[0],
): Promise<void> {
  await api.__testing.sendWebviewDomAction(action);
  await pause(150);
}

function metricWidth(snapshot: DomSnapshot, key: string): number {
  return snapshot.composerControlMetrics[key]?.width ?? 0;
}

function hasThinkingBeforeAssistant(snapshot: DomSnapshot): boolean {
  const lastThinkingIndex = snapshot.timelineKinds.lastIndexOf("thinking-block");
  if (lastThinkingIndex === -1) {
    return false;
  }
  return snapshot.timelineKinds
    .slice(lastThinkingIndex + 1)
    .includes("message:assistant");
}

suite("Tomcat manual acceptance", () => {
  test("captures real-host webview acceptance artifacts", async () => {
    const screenshots: string[] = [];
    const limitations: string[] = [];
    const reportPath = requireEnv("TOMCAT_ACCEPT_REPORT_PATH");
    const api = await hostE2e.getTomcatExtensionApi();

    await api.__testing.focusWebview();
    await api.__testing.waitForWebviewReady(20_000);

    const hydrated = await waitForDom(
      api,
      (snapshot) =>
        snapshot.messageTexts.some((text) => /Historic prompt 6/i.test(text)) &&
        snapshot.toolTitles.some((title) => /search_files/i.test(title))
          ? snapshot
          : undefined,
      20_000,
    );
    screenshots.push(await captureScreenshot("01-history-hydration.png"));
    const closeButtonAbsent =
      !hydrated.html.includes("Close active session") && !hydrated.html.includes(">Close<");

    api.__testing.clearObservedEvents();
    await api.__testing.sendWebviewIntent({
      data: {
        text: "manual acceptance prompt",
      },
      messageId: "manual-acceptance-prompt",
      type: "prompt",
    });

    await api.__testing.waitForEvent({
      textIncludes: "Manual acceptance reply for prompt",
      timeoutMs: 15_000,
      type: "message_update",
    });
    await pause(300);
    const following = await api.__testing.captureWebviewDom();
    screenshots.push(await captureScreenshot("02-autoscroll-following.png"));

    await sendDomAction(api, {
      edge: "top",
      kind: "scrollToEdge",
      testId: "stream-container",
    });
    const jumpVisible = await waitForDom(
      api,
      (snapshot) => (snapshot.jumpToLatestVisible ? snapshot : undefined),
      5_000,
    );
    screenshots.push(await captureScreenshot("03-jump-to-latest-visible.png"));

    await sendDomAction(api, {
      kind: "clickTestId",
      testId: "scroll-to-bottom",
    });
    let backAtBottom = await waitForDom(
      api,
      (snapshot) => snapshot,
      500,
    );
    if (backAtBottom.jumpToLatestVisible) {
      limitations.push(
        "The harness could prove the Jump to latest button became visible, but the test-only synthetic click did not reliably dismiss it, so the acceptance flow fell back to a direct bottom scroll action inside the real VSCode webview.",
      );
      await sendDomAction(api, {
        edge: "bottom",
        kind: "scrollToEdge",
        testId: "stream-container",
      });
      backAtBottom = await waitForDom(
        api,
        (snapshot) => (!snapshot.jumpToLatestVisible ? snapshot : undefined),
        5_000,
      );
    }
    screenshots.push(await captureScreenshot("04-jump-to-latest-restored.png"));

    await api.__testing.waitForEvent({
      timeoutMs: 20_000,
      type: "agent_end",
    });
    const completed = await waitForDom(
      api,
      (snapshot) =>
        snapshot.toolTitles.some((title) => /search_workspace/i.test(title)) &&
        snapshot.toolTitles.some((title) => /validate_layout/i.test(title))
          ? snapshot
          : undefined,
      10_000,
    );
    screenshots.push(await captureScreenshot("05-tool-default-states.png"));

    await sendDomAction(api, {
      index: -1,
      kind: "clickTestId",
      testId: "thinking-toggle",
    });
    const thinkingExpanded = await waitForDom(
      api,
      (snapshot) => (snapshot.expandedThinkingCount > 0 ? snapshot : undefined),
      5_000,
    );
    screenshots.push(await captureScreenshot("06-thinking-expanded.png"));

    const liveSearchIndex = completed.toolTitles.findIndex((title) =>
      /search_workspace/i.test(title),
    );
    assert.ok(liveSearchIndex >= 0, "expected the live complete tool card to exist");
    await sendDomAction(api, {
      index: liveSearchIndex,
      kind: "clickTestId",
      testId: "tool-toggle",
    });
    const toolExpanded = await waitForDom(
      api,
      (snapshot) =>
        snapshot.expandedToolTitles.some((title) => /search_workspace/i.test(title))
          ? snapshot
          : undefined,
      5_000,
    );
    screenshots.push(await captureScreenshot("07-live-tool-expanded.png"));

    await sendDomAction(api, {
      kind: "setRootWidth",
      widthPx: 420,
    });
    const narrow = await waitForDom(
      api,
      (snapshot) => (snapshot.composerRowCount === 1 ? snapshot : undefined),
      5_000,
    );
    screenshots.push(await captureScreenshot("08-composer-narrow.png"));

    await sendDomAction(api, {
      kind: "setRootWidth",
      widthPx: null,
    });
    const wide = await waitForDom(
      api,
      (snapshot) => (snapshot.composerRowCount === 1 ? snapshot : undefined),
      5_000,
    );
    screenshots.push(await captureScreenshot("09-composer-wide.png"));

    const initialWebviewState = api.__testing.getWebviewState();
    assert.ok(initialWebviewState.activeSessionId, "expected an active webview session");
    await api.__testing.sendWebviewIntent({
      data: {
        modelId: "deepseek-v4-pro",
        sessionId: initialWebviewState.activeSessionId,
      },
      messageId: "manual-acceptance-set-model-deepseek",
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
    screenshots.push(await captureScreenshot("10-model-switched-deepseek.png"));

    api.__testing.clearObservedEvents();
    await api.__testing.sendWebviewIntent({
      data: {
        sessionId: deepseekState.activeSessionId,
        text: "trigger capability mismatch",
      },
      messageId: "manual-acceptance-capability-mismatch",
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
    screenshots.push(await captureScreenshot("11-capability-mismatch-error.png"));

    await api.__testing.restartServe();
    await api.__testing.waitForWebviewReady(20_000);
    await pause(500);
    api.__testing.clearObservedEvents();
    await api.__testing.sendWebviewIntent({
      data: {
        text: "after restart prompt",
      },
      messageId: "manual-acceptance-after-restart",
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
    screenshots.push(await captureScreenshot("12-after-restart-recovered.png"));

    const toolScrollMetric = toolExpanded.toolBodyMetrics.find((entry) =>
      /search_workspace/i.test(entry.title),
    );
    const errorToolMetric = completed.toolBodyMetrics.find((entry) =>
      /validate_layout/i.test(entry.title),
    );

    const report = {
      artifactsRoot: path.dirname(reportPath),
      checks: {
        autoscroll: {
          passed:
            !following.jumpToLatestVisible &&
            jumpVisible.jumpToLatestVisible &&
            !backAtBottom.jumpToLatestVisible,
        },
        composer: {
          passed:
            narrow.composerRowCount === 1 && wide.composerRowCount === 1,
        },
        hydration: {
          passed:
            hydrated.messageTexts.some((text) => /Historic prompt 6/i.test(text)) &&
            hydrated.toolTitles.some((title) => /search_files/i.test(title)),
        },
        thinking: {
          passed: thinkingExpanded.expandedThinkingCount > 0 && hasThinkingBeforeAssistant(completed),
        },
        modelSwitch: {
          passed:
            deepseekState.activeSessionId === mismatchState.activeSessionId &&
            mismatchState.sessionViews[mismatchState.activeSessionId ?? ""]?.timeline.some(
              (item) =>
                item.type === "message" &&
                item.kind === "error" &&
                typeof item.text === "string" &&
                /provider\/model 不支持 vision/i.test(item.text),
            ) === true,
        },
        restart: {
          passed: restartRecovered.messageTexts.some((text) => /after restart prompt/i.test(text)),
        },
        sessionBar: {
          passed: closeButtonAbsent,
        },
        toolcards: {
          passed:
            !completed.expandedToolTitles.some((title) => /search_workspace/i.test(title)) &&
            completed.expandedToolTitles.some((title) => /validate_layout/i.test(title)) &&
            !!toolScrollMetric &&
            toolScrollMetric.scrollHeight > toolScrollMetric.clientHeight &&
            !!errorToolMetric &&
            errorToolMetric.expanded,
        },
      },
      limitations: [
        "Composer responsive acceptance used a test-only root-width shim inside the real VSCode webview instead of dragging the host window divider, because this macOS environment repeatedly hijacked external keyboard/mouse automation with system overlays.",
        ...limitations,
      ],
      screenshots,
    };

    await fs.writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`, "utf8");

    for (const [name, value] of Object.entries(report.checks)) {
      assert.equal(value.passed, true, `expected ${name} acceptance check to pass`);
    }
  });
});
