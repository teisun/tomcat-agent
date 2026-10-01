// Render the production App/TipTap with a simulated webview host. Real Serve
// behavior is independently owned by tests/serve_slash_command.test.ts.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { resolveBrowserPath, resolveLaunchOptions } from "../../tomcat/assets/skills/verify/scripts/browser-path.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const verify = new URL("../../tomcat/assets/skills/verify/scripts/shot.mjs", import.meta.url);
const index = process.argv.indexOf("--out");
assert.ok(index >= 0 && process.argv[index + 1], "Pass --out <project_resource_dir>/shots/resource-slash");
const out = path.resolve(process.argv[index + 1]);
await mkdir(out, { recursive: true });
process.env.PLAYWRIGHT_BROWSERS_PATH = resolveBrowserPath(verify.href);
const { chromium } = createRequire(verify)("playwright");
const commands = [
  { name: "reload", usage: "/reload", summary: "重新核对磁盘上的 Skill 与插件" },
  { name: "install", usage: "/install <来源> <current-project|agent|global>", summary: "安装 package / 插件 / Skill" },
  { name: "uninstall", usage: "/uninstall <包名> <current-project|agent|global>", summary: "卸载 package" },
];
function fixture() {
  return {
    activeSessionId: "s1", availableModels: ["gpt-5.4"], ready: true, mediaRoots: [],
    slashCommands: commands,
    sessions: [{ sessionId: "s1", title: "资源命令验收", isCurrent: true, ownedByThisFrontend: true, busy: false, updatedAt: 1 }],
    sessionViews: { s1: {
      sessionId: "s1", ownedByThisFrontend: true, busy: false, commandPending: false,
      model: "gpt-5.4", agentMode: "chat", thinkingLevel: "high", contextRatio: 0.04,
      activePlan: { path: "/fixture/example.plan.md", planId: "plan-1", state: "planning" },
      planTodos: [], sessionTodos: [], pendingAttachments: [],
      timeline: [{ type: "tool", id: "create-1", toolCallId: "call-plan", toolName: "create_plan", isError: false, status: "complete", summary: "created", planId: "plan-1", planPath: "/fixture/example.plan.md", planActivity: { kind: "create", title: "资源命令验收", stateAfter: "planning" } }],
    } },
  };
}
function theme(light) {
  const foreground = light ? "#333333" : "#cccccc";
  const background = light ? "#f3f3f3" : "#181818";
  const input = light ? "#ffffff" : "#202020";
  return `:root { --vscode-font-family: -apple-system, BlinkMacSystemFont, sans-serif; --vscode-font-size: 13px; --vscode-foreground: ${foreground}; --vscode-descriptionForeground: ${light ? "#616161" : "#999999"}; --vscode-sideBar-background: ${background}; --vscode-editor-background: ${background}; --vscode-editorWidget-background: ${input}; --vscode-editorWidget-border: #88888855; --vscode-panel-border: #88888855; --vscode-input-background: ${input}; --vscode-input-foreground: ${foreground}; --vscode-input-placeholderForeground: #888888; --vscode-dropdown-background: ${input}; --vscode-dropdown-foreground: ${foreground}; --vscode-dropdown-border: #88888855; --vscode-list-activeSelectionForeground: ${foreground}; --vscode-list-activeSelectionBackground: ${light ? "#e4e4e4" : "#333333"}; --vscode-button-background: #267fa4; --vscode-button-foreground: #ffffff; --vscode-focusBorder: #267fa4; --vscode-scrollbarSlider-background: #88888866; } body { padding: 0 20px; font-size: 13px; }`;
}
const vite = spawn(process.execPath, [path.join(root, "gui/node_modules/vite/bin/vite.js"), "--host", "127.0.0.1", "--port", "0"], { cwd: path.join(root, "gui"), stdio: ["ignore", "pipe", "pipe"] });
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
  browser = await chromium.launch({ headless: true, ignoreDefaultArgs: ["--hide-scrollbars"], ...await resolveLaunchOptions(verify.href) });
  for (const scenario of [{ name: "narrow-dark", width: 390, height: 844 }, { name: "desktop-light", width: 1440, height: 900, light: true }]) {
    const page = await browser.newPage({ viewport: { width: scenario.width, height: scenario.height } });
    page.setDefaultTimeout(10_000);
    const events = [];
    page.on("console", (message) => events.push({ level: message.type(), text: message.text() }));
    page.on("pageerror", (error) => events.push({ level: "error", text: error.message }));
    await page.addInitScript((state) => {
      window.__fixture = state;
      window.__intents = [];
      window.__emitState = () => window.postMessage({ channel: "state", content: window.__fixture, messageId: `state-${Date.now()}` }, "*");
      let saved;
      window.acquireVsCodeApi = () => ({
        getState: () => saved, setState: (value) => { saved = value; },
        postMessage: (message) => { window.__intents.push(message); if (message.type === "ready") window.__emitState(); },
      });
    }, fixture());
    page.on("requestfailed", (request) => events.push({ level: "warning", url: request.url(), text: request.failure()?.errorText }));
    await page.goto(url, {waitUntil:"domcontentloaded", timeout:60_000}).catch(async (error) => {
      await writeFile(path.join(out, `${scenario.name}-navigation.console.json`), JSON.stringify(events, null, 2));
      throw error;
    });
    await page.addStyleTag({ content: theme(scenario.light) });
    await page.getByTestId("build-plan").waitFor();
    const input = page.getByTestId("composer-input");
    const capture = async (suffix) => {
      const name = `${scenario.name}-${suffix}`;
      await page.screenshot({ path: path.join(out, `${name}.png`) });
      await writeFile(path.join(out, `${name}.aria.txt`), await page.locator("body").ariaSnapshot());
      await writeFile(path.join(out, `${name}.console.json`), JSON.stringify(events, null, 2));
      assert.equal(events.filter((event) => event.level === "error").length, 0, JSON.stringify(events));
      if (suffix === "mid-message" || suffix === "hard-break") {
        const clipped = await page.getByTestId("slash-command-option").evaluateAll((options) => options.filter((option) => {
          const title = option.querySelector(".tc-session-item__title");
          const hint = option.lastElementChild;
          const range = document.createRange();
          range.setStart(title.firstChild, 0);
          range.setEnd(title.firstChild, title.textContent.split(" ")[0].length);
          const roomForEllipsis = title.scrollWidth > title.clientWidth ? 12 : 0;
          return hint.scrollWidth > hint.clientWidth || range.getBoundingClientRect().right > title.getBoundingClientRect().right - roomForEllipsis + 1;
        }).map((option) => option.textContent));
        assert.deepEqual(clipped, [], "command name and leading-only warning must remain readable on narrow screens");
      }
      console.log(`Captured ${name}`);
    };
    const replace = async (text) => {
      await input.click();
      await page.keyboard.press(process.platform === "darwin" ? "Meta+A" : "Control+A");
      await page.keyboard.press("Backspace");
      await page.keyboard.type(text);
    };
    await replace("/");
    await page.getByTestId("slash-command-menu").waitFor();
    assert.equal(await page.getByTestId("slash-command-option").count(), 3);
    if (scenario.light) {
      const colors = await page.getByTestId("slash-command-menu").evaluate((menu) => ({background:getComputedStyle(menu).backgroundColor,foreground:getComputedStyle(menu.querySelectorAll('button')[1]).color}));
      assert.equal(colors.background, "rgb(255, 255, 255)", "light theme must use injected VS Code dropdown tokens");
      assert.equal(colors.foreground, "rgb(51, 51, 51)");
    }
    await capture("menu");
    await page.keyboard.press("Enter");
    assert.equal(await input.innerText(), "/reload ");
    assert.equal(await page.evaluate(() => window.__intents.filter((intent) => intent.type === "runSlashCommand").length), 0);
    await page.getByTestId("send-button").click();
    assert.equal(await page.evaluate(() => window.__intents.filter((intent) => intent.type === "runSlashCommand").at(-1).data.text), "/reload ");
    await page.evaluate(() => { window.__fixture.sessionViews.s1.commandPending = true; window.__emitState(); });
    await page.getByTestId("composer-notice-command").waitFor();
    for (const id of ["send-button", "compact-context-button", "build-plan"]) assert.equal(await page.getByTestId(id).isDisabled(), true);
    await capture("pending");
    await page.evaluate(() => {
      window.__fixture.sessionViews.s1.commandPending = false;
      window.__fixture.sessionViews.s1.timeline.push({ type: "message", id: "reply", kind: "notice", text: "资源已同步：删除 Skill 1、插件 1。" });
      window.__emitState();
    });
    await page.getByText("资源已同步：删除 Skill 1、插件 1。", { exact: false }).waitFor();
    assert.equal(await page.getByTestId("compact-context-button").isDisabled(), false);
    await replace("help /");
    await page.getByTestId("slash-command-menu").waitFor();
    assert.equal(await page.getByText("仅在开头生效").count(), 3);
    await capture("mid-message");
    await page.keyboard.press("Escape");
    assert.equal(await page.getByTestId("slash-command-menu").count(), 0);
    for (const text of ["src/", "https://"]) { await replace(text); assert.equal(await page.getByTestId("slash-command-menu").count(), 0); }
    await replace("line");
    await page.keyboard.press("Shift+Enter");
    await page.keyboard.type("/");
    await page.getByTestId("slash-command-menu").waitFor();
    await capture("hard-break");
    await page.close();
  }
} finally {
  await browser?.close();
  if (vite.exitCode === null) {
    const exited = new Promise((resolve) => vite.once("exit", resolve));
    vite.kill("SIGTERM");
    await exited;
  }
}
