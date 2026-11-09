import * as assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as path from "node:path";
import * as vscode from "vscode";
import type { TomcatExtensionApi } from "../../../extension";
import { TEST_PATH_ENV, TEST_DEFAULT_CWD_ENV } from "../../../constants";
import { startAcpTestHost } from "./acpTestHost";
import { spawnScriptedOpenAiStreamServer, sseDelta, sseToolCall, sseFinish, sseDone } from "./scriptedModel";
import { SettingsFrameDriver } from "./workbenchFindDriver";
import { translate } from "../../../shared/i18n";

async function until(predicate: () => boolean | Promise<boolean>, description: string, timeoutMs = 20_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await predicate()) return;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  throw new Error(`Observation journey timed out: ${description}`);
}

/** Real CLI, real shared service, real webview; only the external LLM is scripted. */
export async function assertSharedObservationFlow(
  getApi: () => Promise<TomcatExtensionApi>, replacement: "reload" | "liveness" = "reload",
): Promise<void> {
  const binary = process.env[TEST_PATH_ENV];
  const cwd = process.env[TEST_DEFAULT_CWD_ENV];
  const artifacts = process.env.TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR;
  const port = Number(process.env.TOMCAT_E2E_OBSERVATION_MODEL_PORT);
  assert.ok(binary && cwd && artifacts && port > 0, "Real observation fixture and artifact directory are required");
  await fs.mkdir(artifacts, { recursive: true });
  const foreignCwd = path.join(path.dirname(cwd), "other-project");
  await fs.mkdir(foreignCwd);
  let release!: () => void;
  const hold = new Promise<void>(resolve => { release = resolve; });
  const prefix = "My progress before VS Code joined.";
  const toolRelease = path.join(cwd, "observer-tool.release");
  const command = "printf running > observer-tool.running; while [ ! -f observer-tool.release ]; do sleep 0.05; done; printf done > observer-tool.done; printf 'tool completed'";
  const model = await spawnScriptedOpenAiStreamServer([
    { parts: [sseDelta(prefix), { body: "", waitFor: hold }, sseToolCall("observer-shell", "bash", JSON.stringify({ command, cwd: "", foreground_wait_ms: 16000, run_in_background: false })), sseFinish("tool_calls"), sseDone()] },
    { parts: [sseDelta("The tool completed. The task is finished."), sseFinish("stop"), sseDone()] },
    { parts: [sseToolCall("observer-question", "ask_question", JSON.stringify({ questions: [{ id: "observer-choice", prompt: "Choose a fixture option.", options: [{ id: "a", label: "Option A", recommended: true }, { id: "b", label: "Option B", recommended: false }] }] })), sseFinish("tool_calls"), sseDone()] },
    { parts: [sseDelta("The observer answered the question."), sseFinish("stop"), sseDone()] },
  ], [], port);
  const host = startAcpTestHost(binary, cwd, process.env);
  let chat: SettingsFrameDriver | undefined;
  try {
    const api = await getApi();
    await api.__testing.focusWebview();
    await api.__testing.waitForWebviewReady();
    await until(() => api.__testing.getWebviewState().ready && !!api.__testing.getWebviewState().activeSessionId, "native window initialization and selected session");
    const ownId = api.__testing.getWebviewState().activeSessionId;
    assert.ok(ownId);
    await host.request("initialize", { protocolVersion: 1, clientCapabilities: {}, clientInfo: { name: "ACP test host", version: "1" } });
    const { sessionId } = await host.request<{ sessionId: string }>("session/new", { cwd, mcpServers: [] });
    const foreign = await host.request<{ sessionId: string }>("session/new", { cwd: foreignCwd, mcpServers: [] });
    const turn = host.request("session/prompt", { sessionId, prompt: [{ type: "text", text: "Inspect this project and run the check." }] }, 180_000);
    void turn.catch(() => undefined);
    await until(() => JSON.stringify(host.updates).includes(prefix), "host prefix before opening");
    // Creating the ACP session changed the disk default. Restore A through its
    // real control path before observing B, which must not change that default.
    await api.__testing.sendWebviewIntent({ type: "switchSession", messageId: "restore-disk-default", data: { sessionId: ownId } });
    chat = await SettingsFrameDriver.connectFromEnvironment(".tc-shell");
    // Capture a real desktop-width panel, not a narrow sidebar inside a wide workbench.
    await api.__testing.executeCommand("workbench.action.maximizeAuxiliaryBar");
    await chat.setViewport(1440, 900);
    await chat.waitForSnapshot(snapshot => snapshot.viewport.width >= 1000, "desktop observer panel is wide");
    await chat.click("session-select");
    await until(() => api.__testing.getWebviewState().sessions.some(row => row.sessionId === sessionId), "ordinary dropdown refresh finds the new host session");
    assert.equal(api.__testing.getWebviewState().sessions.some(row => row.sessionId === foreign.sessionId), false);
    await until(() => chat!.evaluate<boolean>(`[...document.querySelectorAll('[data-testid="session-option"]')].some(row => row.textContent.includes('ACP test host'))`), "host row rendered");
    const index = await chat.evaluate<number>(`[...document.querySelectorAll('[data-testid="session-option"]')].findIndex(row => row.textContent.includes('ACP test host'))`);
    await chat.click("session-option", index);
    await until(() => api.__testing.getWebviewState().activeSessionId === sessionId, "ordinary session switch");
    const state = () => api.__testing.getWebviewState().sessionViews[sessionId];
    await until(() => state()?.timeline.some(item => item.type === "message" && item.text === prefix) === true, "replayed prefix");
    assert.equal(state().access, "observe");
    const readOnlyDom = async (questionPending = false) => {
      const dom = await chat!.evaluate<{ notice: string; editable: string; disabled: string[]; actions: Array<{ disabled: boolean }>; controlsFit: boolean }>(`({
        notice: document.querySelector('[data-testid="observer-notice"]')?.textContent ?? '',
        editable: document.querySelector('[data-testid="composer-input"]')?.getAttribute('contenteditable'),
        disabled: [...document.querySelectorAll('button[disabled]')].map(button => button.dataset.testid),
        actions: [...document.querySelectorAll('[data-testid="stop-button"], [data-testid="send-button"]')].map(button => ({ disabled: button.disabled })),
        controlsFit: [...document.querySelectorAll('[data-testid="mode-select"], [data-testid="model-select"], [data-testid="stop-button"], [data-testid="send-button"]')].every(button => { const rect = button.getBoundingClientRect(); return rect.left >= 0 && rect.right <= innerWidth; })
      })`);
      assert.equal(dom.notice.trim(), translate("en", questionPending ? "session.observerCanAnswer" : "session.readOnlyFrom", { source: "ACP test host" }));
      assert.equal(dom.editable, "false");
      assert.ok(dom.controlsFit, "composer controls fit inside the panel");
      for (const id of ["mode-select", "model-select", "thinking-level-select"]) assert.ok(dom.disabled.includes(id), `${id} must be disabled`);
      // A nonempty preserved draft shows Send even while busy; either action is read-only.
      assert.equal(dom.actions.length, 1);
      assert.equal(dom.actions[0].disabled, true, "The displayed Send/Stop action must be disabled");
    };
    await until(() => chat!.evaluate<boolean>("!!document.querySelector('[data-testid=\"observer-notice\"]')"), "read-only chrome");
    await readOnlyDom();
    const previousDocument = await chat.evaluate<number>("performance.timeOrigin");
    let editor: vscode.TextEditor | undefined;
    if (replacement === "liveness") {
      assert.equal((await api.__testing.listSessions("disk")).activeSessionId, ownId,
        "The disk default must differ from the observed selection");
      await api.__testing.sendWebviewIntent({ type: "syncComposerDraft", messageId: "observer-draft",
        data: { sessionId, text: "Keep the observed session draft", segments: [] },
      });
      await until(() => state().composerDraft?.text === "Keep the observed session draft", "host draft before recovery");
      editor = await vscode.window.showTextDocument(await vscode.workspace.openTextDocument({ content: "Keep editor focus during observation recovery" }));
      await api.__testing.sendWebviewHostEvent({ type: "__test.pause_liveness", enabled: true });
      await until(() => chat!.evaluate<boolean>("document.body.hasAttribute('data-tomcat-liveness-paused') || !!document.querySelector('[data-tomcat-liveness-paused]')"), "paused production liveness");
    }
    const before = state();
    const backendPid = api.__testing.getServeProcessId();
    chat.close(); chat = undefined;
    if (replacement === "reload") {
      const reloadCommand = "workbench.action.webview.reloadWebviewAction";
      assert.ok((await vscode.commands.getCommands(true)).includes(reloadCommand));
      await api.__testing.executeCommand(reloadCommand);
    }
    // The command resolves before Chromium replaces the old iframe. Probe only;
    // never replay the command or a click when its execution context disappears.
    await until(async () => {
      let candidate: SettingsFrameDriver | undefined;
      try {
        candidate = await SettingsFrameDriver.connectFromEnvironment(".tc-shell");
        if (await candidate.evaluate<number>("performance.timeOrigin") !== previousDocument) { chat = candidate; return true; }
      } catch (error) {
        // Discovery can race destruction of the old iframe. Retry only these
        // read-only probes, never the recovery action or a user-visible click.
        if (!(error instanceof Error) || ![
          "CDP -32000: Cannot find context with specified id", "Failed to open CDP websocket",
          "CDP websocket closed",
        ].includes(error.message)) throw error;
      } finally { if (chat !== candidate) candidate?.close(); }
      return false;
    }, "new rendered webview document", replacement === "liveness" ? 30_000 : 20_000);
    // The polling callback assigned this value; TypeScript cannot follow that write.
    chat = chat as SettingsFrameDriver | undefined;
    assert.ok(chat);
    await api.__testing.waitForWebviewReady();
    await until(() => chat!.evaluate<boolean>(`document.body.innerText.includes(${JSON.stringify(prefix)})`), "prefix after webview reload");
    assert.equal(api.__testing.getWebviewState().activeSessionId, sessionId);
    assert.equal(state().access, before.access);
    assert.deepEqual(state().composerDraft, before.composerDraft);
    assert.deepEqual(state().errorRecovery, before.errorRecovery);
    assert.equal(api.__testing.getServeProcessId(), backendPid);
    if (editor) {
      assert.equal(vscode.window.activeTextEditor?.document, editor.document);
      assert.equal(await chat.evaluate("document.hasFocus()"), false, "Recovery must not steal editor focus");
    }
    await readOnlyDom();
    await api.__testing.executeCommand("workbench.action.maximizeAuxiliaryBar");
    await chat.setViewport(1440, 900);
    await chat.capture(path.join(artifacts, "observer-desktop.png"));
    assert.equal(state().timeline.filter(item => item.type === "message" && item.text === prefix).length, 1);
    release();
    await until(() => fs.stat(path.join(cwd, "observer-tool.running")).then(() => true, () => false), "actual tool started");
    await until(() => state().timeline.some(item => item.type === "tool" && item.toolCallId === "observer-shell" && item.status === "running"), "running tool reaches the observer");
    // Narrow the panel, not a whole editor plus sidebar squeezed into 390px.
    await api.__testing.executeCommand("workbench.action.maximizeAuxiliaryBar");
    await chat.setViewport(390, 844);
    await readOnlyDom();
    await chat.capture(path.join(artifacts, "observer-narrow.png"));
    await fs.writeFile(toolRelease, "finish");
    assert.equal((await turn).stopReason, "end_turn");
    assert.equal(await fs.readFile(path.join(cwd, "observer-tool.done"), "utf8"), "done");
    await until(() => !state().busy && state().timeline.some(item => item.type === "message" && item.text.includes("The task is finished.")), "tool and task completion");
    assert.ok(state().timeline.some(item => item.type === "tool" && item.toolCallId === "observer-shell" && item.status === "complete"));
    const questionTurn = host.request("session/prompt", { sessionId, prompt: [{ type: "text", text: "Ask the fixture question." }] }, 180_000);
    void questionTurn.catch(() => undefined);
    await until(() => state().timeline.some(item => item.type === "approval" && item.live && !item.resolved), "observer can answer the host question");
    await chat.setViewport(1440, 900);
    await api.__testing.executeCommand("workbench.action.maximizeAuxiliaryBar");
    await chat.waitForSnapshot(snapshot => snapshot.viewport.width >= 1000, "wide observer question card");
    await until(() => chat!.evaluate<boolean>("!!document.querySelector('[data-testid=\"pending-question-panel\"]')"), "question card rendered");
    await readOnlyDom(true);
    await chat.capture(path.join(artifacts, "observer-question-desktop.png"));
    await chat.setViewport(390, 844);
    await readOnlyDom(true);
    await chat.capture(path.join(artifacts, "observer-question-narrow.png"));
    await chat.click("approval-option-observer-choice-b");
    await chat.click("approval-continue");
    assert.equal((await questionTurn).stopReason, "end_turn");
    await until(() => chat!.evaluate<boolean>("!document.querySelector('[data-testid=\"pending-question-panel\"]')"), "answered question card withdrawn");
    const questionResult = () => state().timeline.find(item => item.type === "tool" && item.toolCallId === "observer-question" && item.status === "complete" && item.summary?.trim().startsWith("{"));
    await until(() => !!questionResult(), "authoritative question result reaches observer");
    const tool = questionResult();
    assert.ok(tool?.type === "tool");
    const answer = JSON.parse(tool.summary ?? "");
    assert.equal(answer.outcome, "answered");
    assert.deepEqual(answer.answers, [{ question_id: "observer-choice", option_ids: ["b"], picked_recommended: false }]);
    await readOnlyDom();
    await host.close();
    // A stopped host is still read-only; no silent ownership transfer.
    await until(() => chat!.evaluate<boolean>("!!document.querySelector('[data-testid=\"send-button\"]') && document.body.innerText.includes('The task is finished.')"), "completed task rendered");
    await readOnlyDom();
    assert.equal(state().access, "observe");
    assert.equal(model.capturedNonTitleRequests().length, 4);
    await assert.rejects(api.__testing.getSessionState(foreign.sessionId), /unknown_session/);
    assert.equal(api.__testing.getObservedEvents().some(event => event.sessionId === foreign.sessionId), false);
    await api.__testing.sendWebviewIntent({ messageId: "observe-return-own", type: "switchSession", data: { sessionId: ownId } });
    await until(() => chat!.evaluate<boolean>("document.querySelector('[data-testid=\"composer-input\"]')?.getAttribute('contenteditable') === 'true'"), "own session becomes writable again");
    assert.deepEqual(api.__testing.getObservedWebviewErrors(), []);
    await fs.writeFile(path.join(artifacts, "observer-journey.json"), JSON.stringify({ sessionId, ownId, foreignSessionId: foreign.sessionId, replacement, modelCalls: model.capturedNonTitleRequests().length, assertions: ["same-project disk list", "foreign project hidden and unreadable", "mid-stream prefix", "webview reload without duplication", "real running and completed tool", "readonly busy and idle", "observer answers non-recommended question without task control", "own session writable"] }, null, 2));
  } finally {
    release();
    await fs.writeFile(toolRelease, "cleanup");
    chat?.close();
    try { await host.close(); } finally { await model.close(); }
  }
}
