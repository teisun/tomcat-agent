// Render the real App with a simulated extension host; this is NOT a backend E2E test.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { resolveBrowserPath, resolveLaunchOptions } from "../../tomcat/assets/skills/verify/scripts/browser-path.mjs";

const extensionRoot = fileURLToPath(new URL("../", import.meta.url));
const verifyUrl = new URL("../../tomcat/assets/skills/verify/scripts/shot.mjs", import.meta.url);
const outIndex = process.argv.indexOf("--out");
assert.ok(outIndex >= 0 && process.argv[outIndex + 1], "Pass --out <artifact-directory>");
const baseline = process.argv.includes("--baseline");
const out = path.resolve(process.argv[outIndex + 1], baseline ? "before" : "after");
await mkdir(out, { recursive: true });
process.env.PLAYWRIGHT_BROWSERS_PATH = resolveBrowserPath(verifyUrl.href);
const { chromium } = createRequire(verifyUrl)("playwright");

const shortQuestion = {
  id: "q1", prompt: "如果多出一天假期，你最想怎么过？",
  options: [
    { id: "a", label: "去户外散步，换换心情", recommended: true },
    { id: "b", label: "宅家看电影或打游戏" },
    { id: "c", label: "约朋友吃顿好吃的" },
  ],
};
const secondQuestion = {
  id: "q2", prompt: "你更喜欢用哪种语言写代码？",
  options: [
    { id: "a", label: "TypeScript", recommended: true },
    { id: "b", label: "Rust" },
    { id: "c", label: "Python" },
  ],
};
const longQuestion = {
  id: "q2", prompt: "在 verify 期间如何避免再次修改无关文件，并保证长选项、多题导航和输入框均可正常使用？".repeat(3),
  options: [
    { id: "a", label: "只验证本次修改范围，保留现有的提问与提交行为。".repeat(3), recommended: true },
    { id: "b", label: "Read every option before continuing, including content below the fold. ".repeat(4) },
    { id: "c", label: "最后一个普通选项：滚到这里并选择。" },
  ],
};
function fixture(questions, stress = false) {
  return {
    activeSessionId: "s1", availableModels: ["gpt-5.4"], modelAdminSupported: false,
    ready: true, mediaRoots: [],
    sessions: [{ sessionId: "s1", title: "提问 UI 验收", isCurrent: true, ownedByThisFrontend: true, busy: stress, updatedAt: 1 }],
    sessionViews: { s1: {
      sessionId: "s1", ownedByThisFrontend: true, busy: stress, model: "gpt-5.4",
      activePlan: null, agentMode: "chat", thinkingLevel: "high", contextRatio: 0.04,
      composerDraft: stress ? {
        text: "保留这份输入草稿\n".repeat(16),
        segments: [{ type: "text", text: "保留这份输入草稿\n".repeat(16) }],
      } : undefined,
      planTodos: [], sessionTodos: stress ? [
        { id: "one", content: "检查提问布局", status: "in_progress" },
        { id: "two", content: "确认滚动与菜单", status: "pending" },
      ] : [],
      pendingAttachments: stress ? [{ id: "file-1", blobSha: "fixture", kind: "file", mimeType: "text/plain", filename: "README.md", label: "README.md", path: "/fixture/README.md" }] : [],
      timeline: [
        { id: "user-1", type: "message", kind: "user", text: "请帮我回答这组问题。" },
        { id: "approval-1", type: "approval", live: true, resolved: false, sessionId: "s1", request: { requestId: "r1", responseEvent: "response-r1", questions } },
      ],
    } },
  };
}
function theme(light, fontSize = 13) {
  return `:root {
    --vscode-font-family: -apple-system, BlinkMacSystemFont, sans-serif;
    --vscode-font-size: ${fontSize}px;
    --vscode-foreground: ${light ? "#333333" : "#cccccc"};
    --vscode-descriptionForeground: ${light ? "#616161" : "#999999"};
    --vscode-sideBar-background: ${light ? "#f3f3f3" : "#181818"};
    --vscode-editor-background: ${light ? "#ffffff" : "#181818"};
    --vscode-editorWidget-background: ${light ? "#f3f3f3" : "#202020"};
    --vscode-editorWidget-border: ${light ? "#c8c8c8" : "#333333"};
    --vscode-panel-border: ${light ? "#c8c8c8" : "#333333"};
    --vscode-input-background: ${light ? "#ffffff" : "#202020"};
    --vscode-input-foreground: ${light ? "#333333" : "#cccccc"};
    --vscode-input-placeholderForeground: #888888;
    --vscode-list-activeSelectionBackground: ${light ? "#e4e4e4" : "#333333"};
    --vscode-scrollbarSlider-background: #88888866;
    --vscode-button-background: #267fa4;
    --vscode-button-foreground: #ffffff;
    --vscode-focusBorder: #267fa4;
    scrollbar-color: var(--vscode-scrollbarSlider-background) var(--vscode-editor-background);
  } body { padding: 0 20px; font-size: var(--vscode-font-size); }`;
}

const vite = spawn(process.execPath, [
  path.join(extensionRoot, "gui/node_modules/vite/bin/vite.js"), "--host", "127.0.0.1", "--port", "0",
], { cwd: path.join(extensionRoot, "gui"), stdio: ["ignore", "pipe", "pipe"] });
let browser;
try {
  const url = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("Vite readiness timed out")), 30_000);
    let output = "";
    vite.stdout.on("data", (chunk) => {
      output += chunk.toString();
      const match = output.match(/http:\/\/127\.0\.0\.1:\d+\//);
      if (match) { clearTimeout(timer); resolve(match[0]); }
    });
    vite.stderr.on("data", (chunk) => process.stderr.write(chunk));
    vite.once("error", (error) => { clearTimeout(timer); reject(error); });
    vite.once("exit", (code) => { clearTimeout(timer); reject(new Error(`Vite exited: ${code}`)); });
  });
  console.log(`Vite ready: ${url}`);
  browser = await chromium.launch({ headless: true, ignoreDefaultArgs: ["--hide-scrollbars"], ...await resolveLaunchOptions(verifyUrl.href) });
  const scenarios = baseline ? [{ name: "narrow-dark", width: 390, height: 844 }] : [
    { name: "narrow-dark", width: 390, height: 844 },
    { name: "two-questions", width: 390, height: 844, two: true },
    { name: "two-questions-600", width: 390, height: 600, two: true },
    { name: "two-questions-640", width: 390, height: 640, two: true },
    { name: "large-font", width: 520, height: 1000, two: true, fontSize: 18 },
    { name: "narrow-large-font", width: 320, height: 600, two: true, fontSize: 18, overflow: true },
    { name: "wide-light", width: 1440, height: 900, light: true },
    { name: "short-long", width: 390, height: 280, long: true },
    { name: "with-composer", width: 440, height: 640, stress: true },
  ];
  const results = [];
  for (const scenario of scenarios) {
    const page = await browser.newPage({ viewport: { width: scenario.width, height: scenario.height } });
    page.setDefaultTimeout(10_000);
    const events = [];
    page.on("console", (message) => events.push({ level: message.type(), text: message.text() }));
    page.on("pageerror", (error) => events.push({ level: "error", text: error.message }));
    const state = fixture(scenario.long ? [shortQuestion, longQuestion] : scenario.two ? [shortQuestion, secondQuestion] : [shortQuestion], scenario.stress);
    await page.addInitScript((state) => {
      window.__answers = [];
      window.__fixture = state;
      window.__emitState = () => window.postMessage({ channel: "state", content: window.__fixture, messageId: `state-${Date.now()}` }, "*");
      let saved;
      window.acquireVsCodeApi = () => ({
        getState: () => saved, setState: (value) => { saved = value; },
        postMessage: (message) => {
          if (message.type === "ready") window.__emitState();
          if (message.type === "answerQuestion") window.__answers.push(message.data);
        },
      });
    }, state);
    await page.goto(url);
    await page.addStyleTag({ content: theme(scenario.light, scenario.fontSize) });
    await page.getByTestId("approval-option-q1-a").waitFor();
    await page.evaluate(() => document.fonts.ready);
    const assertQuestionVisible = async (questionId) => {
      assert.equal(await page.getByRole("radiogroup").count(), 1, "Only the current question may be mounted");
      const visibility = await page.getByTestId("approval-questions-body").evaluate((body, id) => {
        const rect = body.getBoundingClientRect();
        const radios = [...body.querySelectorAll(`[data-testid^="approval-option-${id}-"]`)];
        const prompt = radios[0].closest(".tc-approval-question").querySelector(".tc-approval-question__prompt");
        return [prompt, ...radios].map(el => {
          const r = el.getBoundingClientRect();
          return { text: el.textContent, visible: r.top >= rect.top - 1 && r.bottom <= rect.bottom + 1 };
        });
      }, questionId);
      assert.ok(visibility.every(row => row.visible), `${scenario.name}/${questionId}: ${JSON.stringify(visibility)}`);
      return visibility;
    };
    const capture = async (suffix) => {
      const name = `${scenario.name}-${suffix}`;
      await page.screenshot({ path: path.join(out, `${name}.png`) });
      await writeFile(path.join(out, `${name}.aria.txt`), await page.locator("body").ariaSnapshot());
      await writeFile(path.join(out, `${name}.console.json`), JSON.stringify(events, null, 2));
      assert.equal(events.filter((e) => e.level === "error").length, 0, JSON.stringify(events));
      if (!baseline && suffix !== "completed") {
        assert.equal(await page.getByRole("radiogroup").count(), 1);
        assert.equal(await page.locator(".tc-approval-question").evaluate(q => getComputedStyle(q).borderTopWidth), "0px");
        const navigationFits = await page.locator(".tc-approval-card__navigation").evaluate(nav => {
          const panel = nav.closest(".tc-pending-question-panel").getBoundingClientRect();
          return [...nav.querySelectorAll("button")].every(button => {
            const r = button.getBoundingClientRect();
            return r.left >= panel.left && r.right <= panel.right;
          });
        });
        assert.ok(navigationFits, "Navigation and collapse controls must not be clipped horizontally");
        const height = await page.getByTestId("pending-question-panel").evaluate((panel) => ({
          actual: panel.getBoundingClientRect().height,
          limit: Math.min(innerHeight * 0.3, parseFloat(getComputedStyle(document.documentElement).fontSize) * 20),
        }));
        assert.ok(height.actual <= height.limit + 1, `Panel exceeded the original height budget: ${JSON.stringify(height)}`);
      }
    };
    try {
      const geometry = await page.evaluate(() => {
        const panel = document.querySelector('[data-testid="pending-question-panel"]');
        const body = document.querySelector('[data-testid="approval-questions-body"]');
        const option = document.querySelector('[data-testid="approval-option-q1-a"]');
        const cs = getComputedStyle(panel);
        const scroller = body;
        const rect = scroller.getBoundingClientRect();
        return {
          borders: [cs.borderTopWidth, cs.borderRightWidth, cs.borderBottomWidth, cs.borderLeftWidth],
          borderColor: cs.borderTopColor, panelRadius: cs.borderRadius,
          panelHeight: panel.getBoundingClientRect().height,
          panelHeightLimit: Math.min(innerHeight * 0.3, parseFloat(getComputedStyle(document.documentElement).fontSize) * 20),
          scrollContainer: scroller === panel ? "panel" : "body",
          optionRadius: getComputedStyle(option).borderRadius,
          composerRadius: getComputedStyle(document.querySelector('[data-testid="composer-surface"]')).borderRadius,
          bodyHeight: body.clientHeight, bodyScrollHeight: body.scrollHeight,
          outerOverflow: panel.scrollHeight > panel.clientHeight,
          optionCodeSize: option.querySelector(".tc-approval-option__code").getBoundingClientRect().width,
          optionLineHeight: parseFloat(getComputedStyle(option.querySelector(".tc-approval-option__content")).lineHeight),
          optionLabelFontSize: parseFloat(getComputedStyle(option.querySelector(".tc-approval-option__label")).fontSize),
          scrollbarGutter: scroller.offsetWidth - scroller.clientWidth,
          optionsInitiallyVisible: [...document.querySelectorAll('[role="radio"]')].map((el) => {
            const r = el.getBoundingClientRect();
            return r.top >= rect.top && r.bottom <= rect.bottom;
          }),
          horizontalOverflow: document.documentElement.scrollWidth > innerWidth,
        };
      });
      results.push({ scenario: scenario.name, ...geometry });
      await capture("initial");
      if (baseline) continue;
      assert.deepEqual(geometry.borders, ["1px", "1px", "1px", "1px"]);
      assert.notEqual(geometry.borderColor, "rgba(0, 0, 0, 0)");
      assert.equal(geometry.panelRadius, "0px");
      assert.equal(geometry.optionRadius, "2px");
      assert.equal(geometry.composerRadius, "4px");
      assert.equal(geometry.horizontalOverflow, false);
      assert.ok(geometry.scrollbarGutter > 0, "Scrollbar must not disappear into the host's overlay style");
      assert.equal(geometry.scrollContainer, "body");
      assert.equal(await page.getByRole("radiogroup").count(), 1);
      assert.equal(await page.getByTestId("approval-option-q2-a").count(), 0);
      if (!scenario.long && !scenario.stress && !scenario.overflow) results.at(-1).firstQuestionVisibility = await assertQuestionVisible("q1");
      assert.equal(await page.getByTestId("approval-continue").isDisabled(), true);
      await page.getByTestId("approval-option-q1-b").click();
      assert.equal(await page.getByTestId("approval-option-q1-b").getAttribute("aria-checked"), "true");
      if (scenario.two) {
        assert.equal(await page.getByTestId("approval-continue").isDisabled(), true);
        await page.getByTestId("approval-next-question").click();
        assert.equal(await page.getByTestId("approval-option-q1-b").count(), 0);
        if (!scenario.overflow) results.at(-1).secondQuestionVisibility = await assertQuestionVisible("q2");
        await page.getByTestId("approval-option-q2-b").click();
        await capture("second-question");
      }
      if (scenario.long) {
        await page.getByTestId("approval-next-question").click();
        assert.equal(await page.getByTestId("approval-question-count").textContent(), "2 of 2");
        assert.equal(await page.getByTestId("approval-option-q1-b").count(), 0);
        await page.getByTestId("approval-option-q2-c").scrollIntoViewIfNeeded();
        await page.getByTestId("approval-option-q2-c").click();
        await page.getByTestId("approval-option-q2-__custom__").click();
        assert.equal(await page.getByTestId("approval-continue").isDisabled(), true);
        assert.ok(await page.getByTestId("approval-custom-q2").evaluate(input => {
          const r = input.getBoundingClientRect();
          const body = input.closest(".tc-approval-questions--pending").getBoundingClientRect();
          const panel = input.closest(".tc-pending-question-panel").getBoundingClientRect();
          return document.activeElement === input
            && r.top >= Math.max(0, body.top, panel.top) - 1
            && r.bottom <= Math.min(innerHeight, body.bottom, panel.bottom) + 1;
        }), "Other must focus and reveal its field before a test fills or scrolls it");
        await page.getByTestId("approval-custom-q2").fill("  自定义答案  ");
        await page.getByTestId("approval-custom-q2").scrollIntoViewIfNeeded();
        await capture("custom");
      }
      if (scenario.overflow) {
        await page.getByTestId("approval-option-q2-__custom__").scrollIntoViewIfNeeded();
        assert.equal(await page.getByTestId("approval-question-count").textContent(), "2 of 2");
        await capture("overflow-last-option");
      }
      if (scenario.stress) {
        await page.getByTestId("todo-widget-toggle").click();
        assert.equal(await page.getByTestId("composer-input").getAttribute("contenteditable"), "false");
        await capture("busy-draft");
        // Busy sessions intentionally lock Composer; test its menu only after the host unlocks it.
        await page.evaluate(() => {
          window.__fixture.sessionViews.s1.busy = false;
          window.__fixture.sessions[0].busy = false;
          window.__emitState();
        });
        await page.getByTestId("mode-select").click();
        await capture("menu");
        await page.keyboard.press("Escape");
      }
      await page.getByTestId("approval-collapse").click();
      await page.getByTestId("approval-collapse").click();
      if (scenario.two || scenario.long) {
        assert.equal(await page.getByTestId("approval-question-count").textContent(), "2 of 2");
        await page.getByTestId("approval-previous-question").click();
        assert.equal(await page.getByTestId("approval-option-q2-a").count(), 0);
        assert.equal(await page.getByTestId("approval-questions-body").evaluate(body => body.scrollTop), 0);
      }
      assert.equal(await page.getByTestId("approval-option-q1-b").getAttribute("aria-checked"), "true");
      assert.equal(await page.getByTestId("approval-continue").isEnabled(), true);
      await capture("selected");
      if (scenario.name === "narrow-dark") await page.getByTestId("approval-continue").press("Enter");
      else await page.getByTestId("approval-continue").click();
      const answers = await page.evaluate(() => window.__answers);
      assert.equal(answers.length, 1);
      assert.equal(answers[0].result.answers[0].optionIds[0], "b");
      if (scenario.long) assert.equal(answers[0].result.answers[1].customText, "自定义答案");
      if (scenario.two) assert.equal(answers[0].result.answers[1].optionIds[0], "b");
      await page.evaluate(() => {
        window.__composerNode = document.querySelector('[data-testid="composer-input"]');
        window.__fixture.sessionViews.s1.timeline = [];
        window.__emitState();
      });
      await page.getByTestId("pending-question-panel").waitFor({ state: "detached" });
      assert.equal(await page.evaluate(() => window.__composerNode === document.querySelector('[data-testid="composer-input"]')), true);
      await capture("completed");
      if (scenario.stress) assert.ok((await page.getByTestId("composer-input").textContent()).includes("保留这份输入草稿"));
      results.at(-1).passed = true;
    } catch (error) {
      await capture("failed");
      throw error;
    } finally {
      await writeFile(path.join(out, "checks.json"), JSON.stringify({ simulatedHost: true, baseline, results }, null, 2));
      await page.close();
    }
  }
  if (!baseline) {
    const normal = results.find(r => r.scenario === "two-questions");
    const large = results.find(r => r.scenario === "large-font");
    assert.ok(large.optionCodeSize > normal.optionCodeSize * 1.25, "Letter box must scale with the user's font");
    assert.ok(large.optionLineHeight > normal.optionLineHeight * 1.25, "Option line height must scale with the user's font");
    assert.ok(large.optionLabelFontSize > normal.optionLabelFontSize * 1.25, "Option text must scale with the user's font");
  }
  console.log(`Captured ${results.length} scenarios: ${out}`);
} finally {
  await browser?.close();
  if (vite.exitCode === null) {
    const exited = new Promise((resolve) => vite.once("exit", resolve));
    vite.kill("SIGTERM");
    await exited;
  }
}
