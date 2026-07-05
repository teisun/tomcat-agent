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

type TomcatApi = {
  __testing: {
    captureWebviewDom(): Promise<{
      activeSessionId: string | null;
      approvalCount: number;
      approvalInputTestIds: string[];
      approvalOptionStates: Array<{
        selected: boolean;
        testId: string;
      }>;
      disabledTestIds: string[];
      html: string;
      messageTexts: string[];
    }>;
    clearObservedEvents(): void;
    focusWebview(): Promise<void>;
    getWebviewState(): {
      activeSessionId: string | null;
    };
    sendWebviewDomAction(action: {
      edge?: "bottom" | "top";
      index?: number;
      kind: "clickTestId" | "scrollToEdge" | "setInputValue" | "setRootWidth";
      testId?: string;
      value?: string;
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

async function sendDomAction(
  api: TomcatApi,
  action: Parameters<TomcatApi["__testing"]["sendWebviewDomAction"]>[0],
): Promise<void> {
  await api.__testing.sendWebviewDomAction(action);
  await pause(150);
}

suite("Tomcat ask_question reverify", () => {
  test("reverifies ask_question UX in a real VSCode host", async () => {
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
    screenshots.push(await captureScreenshot("01-ask-question-hydrated.png"));

    const initialState = api.__testing.getWebviewState();
    assert.ok(initialState.activeSessionId, "expected an active webview session");

    api.__testing.clearObservedEvents();
    await api.__testing.sendWebviewIntent({
      data: {
        sessionId: initialState.activeSessionId,
        text: "ask question reverify continue path",
      },
      messageId: "ask-question-reverify-continue",
      type: "prompt",
    });

    const initialApproval = await waitForDom(
      api,
      (snapshot) =>
        snapshot.approvalCount === 1 &&
        snapshot.html.includes("你更喜欢在什么时候写代码?") &&
        snapshot.html.includes("白天") &&
        snapshot.html.includes("晚上") &&
        snapshot.html.includes("TypeScript") &&
        snapshot.html.includes("Rust") &&
        snapshot.html.includes("Other...")
          ? snapshot
          : undefined,
      10_000,
    );
    screenshots.push(await captureScreenshot("02-ask-question-initial.png"));

    await sendDomAction(api, {
      kind: "clickTestId",
      testId: "approval-option-q1-day",
    });
    const firstSelection = await waitForDom(
      api,
      (snapshot) =>
        snapshot.approvalOptionStates.some(
          (entry) => entry.testId === "approval-option-q1-day" && entry.selected,
        )
          ? snapshot
          : undefined,
      5_000,
    );
    screenshots.push(await captureScreenshot("03-ask-question-first-selection.png"));

    await sendDomAction(api, {
      kind: "clickTestId",
      testId: "approval-option-q2-__custom__",
    });
    const customVisible = await waitForDom(
      api,
      (snapshot) =>
        snapshot.approvalInputTestIds.includes("approval-custom-q2") ? snapshot : undefined,
      5_000,
    );
    screenshots.push(await captureScreenshot("04-ask-question-custom-empty.png"));

    await sendDomAction(api, {
      kind: "setInputValue",
      testId: "approval-custom-q2",
      value: "Go",
    });
    const readyToContinue = await waitForDom(
      api,
      (snapshot) =>
        !snapshot.disabledTestIds.includes("approval-continue") ? snapshot : undefined,
      5_000,
    );
    screenshots.push(await captureScreenshot("05-ask-question-ready-to-continue.png"));

    await sendDomAction(api, {
      kind: "clickTestId",
      testId: "approval-continue",
    });
    await api.__testing.waitForEvent({
      timeoutMs: 15_000,
      type: "agent_end",
    });
    const continued = await waitForDom(
      api,
      (snapshot) =>
        snapshot.approvalCount === 0 &&
        snapshot.messageTexts.some((text) => /Manual ask_question answers received:/i.test(text))
          ? snapshot
          : undefined,
      10_000,
    );
    screenshots.push(await captureScreenshot("06-ask-question-continued.png"));

    api.__testing.clearObservedEvents();
    await api.__testing.sendWebviewIntent({
      data: {
        sessionId: initialState.activeSessionId,
        text: "ask question reverify skip path",
      },
      messageId: "ask-question-reverify-skip",
      type: "prompt",
    });
    const skipApproval = await waitForDom(
      api,
      (snapshot) =>
        snapshot.approvalCount === 1 &&
        snapshot.disabledTestIds.includes("approval-continue")
          ? snapshot
          : undefined,
      10_000,
    );
    screenshots.push(await captureScreenshot("07-ask-question-skip-prompt.png"));

    await sendDomAction(api, {
      kind: "clickTestId",
      testId: "approval-skip",
    });
    await api.__testing.waitForEvent({
      timeoutMs: 15_000,
      type: "agent_end",
    });
    const skipped = await waitForDom(
      api,
      (snapshot) =>
        snapshot.approvalCount === 0 &&
        snapshot.messageTexts.some((text) => /Manual ask_question skipped/i.test(text))
          ? snapshot
          : undefined,
      10_000,
    );
    screenshots.push(await captureScreenshot("08-ask-question-skipped.png"));

    const report = {
      artifactsRoot: path.dirname(reportPath),
      checks: {
        continueInitiallyDisabled: {
          passed: initialApproval.disabledTestIds.includes("approval-continue"),
        },
        continueRequiresAllAnswers: {
          passed:
            firstSelection.disabledTestIds.includes("approval-continue") &&
            customVisible.disabledTestIds.includes("approval-continue"),
        },
        customInputAppears: {
          passed: customVisible.approvalInputTestIds.includes("approval-custom-q2"),
        },
        optionMatrixVisible: {
          passed:
            initialApproval.html.includes("白天") &&
            initialApproval.html.includes("晚上") &&
            initialApproval.html.includes("TypeScript") &&
            initialApproval.html.includes("Rust") &&
            initialApproval.html.includes("Other..."),
        },
        selectionStateVisible: {
          passed:
            firstSelection.approvalOptionStates.some(
              (entry) => entry.testId === "approval-option-q1-day" && entry.selected,
            ) &&
            customVisible.approvalOptionStates.some(
              (entry) => entry.testId === "approval-option-q2-__custom__" && entry.selected,
            ),
        },
        skipPathCompletes: {
          passed: skipped.messageTexts.some((text) => /Manual ask_question skipped/i.test(text)),
        },
        submitAfterCustomText: {
          passed:
            !readyToContinue.disabledTestIds.includes("approval-continue") &&
            continued.messageTexts.some((text) => /q1=白天/i.test(text)) &&
            continued.messageTexts.some((text) => /q2=Go/i.test(text)),
        },
      },
      screenshots,
    };

    await fs.writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`, "utf8");

    for (const [name, value] of Object.entries(report.checks)) {
      assert.equal(value.passed, true, `expected ${name} ask_question reverify check to pass`);
    }
  });
});
