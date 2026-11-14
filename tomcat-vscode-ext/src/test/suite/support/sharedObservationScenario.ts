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
  const cancelledRelease = path.join(cwd, "cancelled-tool.release");
  const readPlan = sseToolCall("read-plan", "read", "{}");
  const revisePlan = sseToolCall("revise-plan", "edit", "{}");
  const command = "printf running > observer-tool.running; while [ ! -f observer-tool.release ]; do sleep 0.05; done; printf done > observer-tool.done; printf 'tool completed'";
  let releaseResumedPrompt!: () => void;
  const resumedPromptHold = new Promise<void>(resolve => { releaseResumedPrompt = resolve; });
  const resumedPrompt = "Ask the fixture question. Continue in the same conversation after reconnecting, preserve the earlier discussion, and show this complete instruction before generating any reply.";
  const model = await spawnScriptedOpenAiStreamServer([
    { parts: [sseDelta(prefix), { body: "", waitFor: hold }, sseToolCall("observer-shell", "bash", JSON.stringify({ command, cwd: "", foreground_wait_ms: 16000, run_in_background: false })), sseFinish("tool_calls"), sseDone()] },
    { parts: [sseDelta("The tool completed. The task is finished."), sseFinish("stop"), sseDone()] },
    { parts: [{ body: "", waitFor: resumedPromptHold }, sseToolCall("observer-question", "ask_question", JSON.stringify({ questions: [{ id: "observer-choice", prompt: "Choose a fixture option.", options: [{ id: "a", label: "Option A", recommended: true }, { id: "b", label: "Option B", recommended: false }] }] })), sseFinish("tool_calls"), sseDone()] },
    { parts: [sseDelta("The observer answered the question."), sseFinish("stop"), sseDone()] },
    { parts: [sseToolCall("cancelled-shell", "bash", JSON.stringify({ command: command.replaceAll("observer-tool", "cancelled-tool"), cwd: "", foreground_wait_ms: 16000, run_in_background: false })), sseFinish("tool_calls"), sseDone()] },
    { parts: [sseToolCall("create-plan", "create_plan", JSON.stringify({ goal: "Shared revision fixture", draft: "## Approach\nImplement before testing.", todos: [{ id: "implement", content: "Implement after agreement", status: "pending" }] })), sseFinish("tool_calls"), sseDone()] },
    { parts: [sseDelta("Plan ready for your revision."), sseFinish("stop"), sseDone()] },
    { parts: [readPlan, sseFinish("tool_calls"), sseDone()] },
    { parts: [revisePlan, sseFinish("tool_calls"), sseDone()] },
    { parts: [sseDelta("Plan revised in place."), sseFinish("stop"), sseDone()] },
  ], [], port);
  let host = startAcpTestHost(binary, cwd, process.env);
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
    const sharedInputDom = async () => {
      const dom = await chat!.evaluate<{ notice: string; editable: string; disabled: string[]; actions: Array<{ disabled: boolean }>; controlsFit: boolean }>(`({
        notice: document.querySelector('[data-testid="observer-notice"]')?.textContent ?? '',
        editable: document.querySelector('[data-testid="composer-input"]')?.getAttribute('contenteditable'),
        disabled: [...document.querySelectorAll('button[disabled]')].map(button => button.dataset.testid),
        actions: [...document.querySelectorAll('[data-testid="stop-button"], [data-testid="send-button"]')].map(button => ({ disabled: button.disabled })),
        controlsFit: [...document.querySelectorAll('[data-testid="mode-select"], [data-testid="model-select"], [data-testid="stop-button"], [data-testid="send-button"]')].every(button => { const rect = button.getBoundingClientRect(); return rect.left >= 0 && rect.right <= innerWidth; })
      })`);
      assert.equal(dom.notice.trim(), state().busy && state().access === "observe" ? translate("en", "session.runningIn", { host: "ACP test host" }) : "");
      assert.equal(dom.editable, "true");
      assert.ok(dom.controlsFit, "composer controls fit inside the panel");
      for (const id of ["mode-select", "model-select", "thinking-level-select"]) assert.equal(dom.disabled.includes(id), state().busy, `${id} follows busy`);
      assert.equal(dom.actions.length, 1);
      if (state().busy) assert.equal(dom.actions[0].disabled, false, "The displayed Send/Stop action is available");
    };
    await until(() => chat!.evaluate<boolean>("!!document.querySelector('[data-testid=\"observer-notice\"]')"), "shared-input chrome");
    await sharedInputDom();
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
    await sharedInputDom();
    await api.__testing.executeCommand("workbench.action.maximizeAuxiliaryBar");
    await chat.setViewport(1440, 900);
    await chat.fill('[data-testid="composer-input"]', "Test before implementing.");
    await chat.click("send-button");
    await until(() => state().messageQueue?.items.some(item => item.text === "Test before implementing." && item.status === "queued") === true, "ordinary Send queues during the host turn");
    await chat.capture(path.join(artifacts, "observer-desktop.png"));
    await chat.setViewport(390, 844);
    await sharedInputDom();
    await chat.capture(path.join(artifacts, "observer-queued-narrow.png"));
    await chat.click("queue-send");
    await until(() => state().messageQueue?.items.some(item => item.status === "steering") === true, "Send now steers without taking control");
    assert.equal(state().access, "observe");
    assert.equal(state().timeline.filter(item => item.type === "message" && item.text === prefix).length, 1);
    release();
    await until(() => fs.stat(path.join(cwd, "observer-tool.running")).then(() => true, () => false), "actual tool started");
    await until(() => state().timeline.some(item => item.type === "tool" && item.toolCallId === "observer-shell" && item.status === "running"), "running tool reaches the observer");
    // Narrow the panel, not a whole editor plus sidebar squeezed into 390px.
    await api.__testing.executeCommand("workbench.action.maximizeAuxiliaryBar");
    await chat.setViewport(390, 844);
    await sharedInputDom();
    await chat.capture(path.join(artifacts, "observer-narrow.png"));
    await fs.writeFile(toolRelease, "finish");
    assert.equal((await turn).stopReason, "end_turn");
    assert.equal(await fs.readFile(path.join(cwd, "observer-tool.done"), "utf8"), "done");
    assert.ok(model.capturedNonTitleRequests()[1].includes("Test before implementing."), "the host model receives steering at the safe point");
    await until(() => !state().busy && state().timeline.some(item => item.type === "message" && item.text.includes("The task is finished.")), "tool and task completion");
    assert.ok(state().timeline.some(item => item.type === "tool" && item.toolCallId === "observer-shell" && item.status === "complete"));
    // Reconnect the external host while VS Code stays on the original view.
    await host.close();
    host = startAcpTestHost(binary, cwd, process.env);
    await host.request("initialize", { protocolVersion: 1, clientCapabilities: {}, clientInfo: { name: "ACP test host", version: "1" } });
    await host.request("session/load", { sessionId, cwd, mcpServers: [] });
    const keptDraft = "Keep this local draft while ACP resumes.";
    await chat.fill('[data-testid="composer-input"]', keptDraft);
    await until(() => state().composerDraft?.text === keptDraft, "local draft synchronized before remote input");
    const questionTurn = host.request("session/prompt", { sessionId, prompt: [{ type: "text", text: resumedPrompt }] }, 180_000);
    void questionTurn.catch(() => undefined);
    await until(() => model.capturedNonTitleRequests().length === 3, "resumed model paused before first reply");
    const livePromptCount = () => chat!.evaluate<number>(`[...document.querySelectorAll('[data-message-kind="user"] [data-testid="message-text"]')].filter(row => row.textContent === ${JSON.stringify(resumedPrompt)}).length`);
    await until(async () => await livePromptCount() === 1, "resumed prompt visible once before the first reply");
    assert.equal(state().timeline.some(item => item.type === "approval" && item.live && !item.resolved), false, "model is still paused before the question");
    for (const [width, height, name] of [[1440, 900, "desktop"], [390, 844, "narrow"]] as const) {
      await api.__testing.executeCommand("workbench.action.maximizeAuxiliaryBar");
      await chat.setViewport(width, height);
      assert.equal(await livePromptCount(), 1);
      assert.equal(state().composerDraft?.text, keptDraft);
      assert.equal(api.__testing.getWebviewState().activeSessionId, sessionId);
      await chat.capture(path.join(artifacts, `acp-live-prompt-${name}.png`));
    }
    releaseResumedPrompt();
    await until(() => state().timeline.some(item => item.type === "approval" && item.live && !item.resolved), "observer can answer the host question");
    await chat.setViewport(1440, 900);
    await api.__testing.executeCommand("workbench.action.maximizeAuxiliaryBar");
    await chat.waitForSnapshot(snapshot => snapshot.viewport.width >= 1000, "wide observer question card");
    await until(() => chat!.evaluate<boolean>("!!document.querySelector('[data-testid=\"pending-question-panel\"]')"), "question card rendered");
    await sharedInputDom();
    await chat.capture(path.join(artifacts, "observer-question-desktop.png"));
    await chat.setViewport(390, 844);
    await sharedInputDom();
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
    await sharedInputDom();
    assert.equal(state().composerDraft?.text, keptDraft, "answering the remote question also keeps the local draft");
    // The existing Stop journey starts with an empty composer; otherwise its
    // primary action intentionally remains Send (queue this draft).
    await chat.fill('[data-testid="composer-input"]', "");
    await until(() => !state().composerDraft?.text, "clear the fixture draft before testing Stop");
    const stoppedTurn = host.request("session/prompt", { sessionId, prompt: [{ type: "text", text: "Start the cancellable check." }] }, 180_000);
    void stoppedTurn.catch(() => undefined);
    await until(() => fs.stat(path.join(cwd, "cancelled-tool.running")).then(() => true, () => false), "cancellable tool is really running");
    await until(() => state().busy && state().timeline.some(item => item.type === "tool" && item.toolCallId === "cancelled-shell" && item.status === "running"), "running tool is visible before Stop");
    await chat.click("stop-button");
    assert.equal((await stoppedTurn).stopReason, "cancelled");
    await until(() => !state().busy, "stopped round finished cleanup");
    assert.equal(await fs.stat(path.join(cwd, "cancelled-tool.done")).then(() => true, () => false), false);

    // create_plan opens the editor through the normal UI. Restore desktop
    // space before that transition; a 390px workbench cannot show two panels.
    await chat.setViewport(1440, 900);
    await host.request("session/set_config_option", { sessionId, configId: "agent_mode", value: "plan" });
    await host.request("session/set_config_option", { sessionId, configId: "plan_review", value: "skip" });
    const planned = await host.request("session/prompt", { sessionId, prompt: [{ type: "text", text: "Create the fixture plan without implementing it." }] }, 180_000);
    assert.equal(planned.stopReason, "end_turn");
    assert.equal(await livePromptCount(), 1, "later reply does not duplicate the prompt");
    await until(() => !state().busy, "plan is ready to revise");
    const created = api.__testing.getObservedEvents().find(event => event.type === "plan.create" && event.sessionId === sessionId);
    assert.ok(created?.type === "plan.create" && created.path && created.planId);
    const planPath = created.path, planId = created.planId;
    assert.ok((await fs.readFile(planPath, "utf8")).includes("Implement before testing."));
    await host.close();
    await api.__testing.focusWebview();
    await api.__testing.executeCommand("workbench.action.maximizeAuxiliaryBar");
    await chat.waitForSnapshot(snapshot => snapshot.viewport.width >= 1000, "chat restored after automatic plan preview");
    assert.equal(state().access, "observe", "disconnect does not itself transfer control");
    await sharedInputDom();
    readPlan.body = sseToolCall("read-plan", "read", JSON.stringify({ path: planPath })).body;
    revisePlan.body = sseToolCall("revise-plan", "edit", JSON.stringify({ path: planPath, edits: [{ old_content: "Implement before testing.", new_content: "Test before implementation." }] })).body;
    await chat.setViewport(1440, 900);
    await api.__testing.openPlanPreview(planPath);
    const preview = await SettingsFrameDriver.connectFromEnvironment(".tc-plan-preview__body");
    try {
      await until(() => preview.evaluate<boolean>("document.body.innerText.includes('Implement before testing.')"), "real ACP plan preview");
      await preview.evaluate(`(() => { const paragraph = [...document.querySelectorAll('.tc-plan-preview__body p')].find(p => p.textContent.includes('Implement before testing.')); if (!paragraph) throw new Error('plan paragraph missing'); paragraph.id = 'revision-quote'; })()`);
      await api.__testing.dispatchPlanPreviewDomAction(planPath, { kind: "selectText", selector: "#revision-quote" });
      await until(() => preview.evaluate<boolean>("!!document.querySelector('[data-testid=\"plan-selection-add\"]')"), "plan quote action");
      await api.__testing.dispatchPlanPreviewDomAction(planPath, { kind: "clickSelectionAdd" });
      await until(() => chat!.evaluate<boolean>("!!document.querySelector('[data-testid=\"composer-reference-chip\"]')"), "plan quote reaches the same session");
      await api.__testing.sendWebviewDomAction({ kind: "setInputValue", testId: "composer-input", value: " Change the quoted step: test first." });
      for (const [width, height, name] of [[1440, 900, "desktop"], [390, 844, "narrow"]] as const) {
        await api.__testing.executeCommand("workbench.action.maximizeAuxiliaryBar");
        await chat.setViewport(width, height);
        await sharedInputDom();
        await chat.capture(path.join(artifacts, `shared-plan-quote-${name}.png`));
      }
      await chat.click("send-button");
      await until(() => state().access === "control" && !state().busy && state().timeline.some(item => item.type === "message" && item.text.includes("Plan revised in place.")), "native revision finishes in the same session");
      const revised = await fs.readFile(planPath, "utf8");
      assert.ok(revised.includes(`plan_id: ${planId}`) && revised.includes("Test before implementation."));
      assert.equal(state().agentMode, "plan", "revision must not start execution");
      const revisionInput = JSON.stringify(JSON.parse(model.capturedNonTitleRequests()[7].split("\r\n\r\n")[1]).messages
        .filter((message: { role: string }) => message.role === "user"));
      assert.ok(revisionInput.includes("Implement before testing."), "the selected plan text reaches the model as user input");
      assert.ok(revisionInput.includes("Change the quoted step: test first."), "the revision instruction reaches the model with the quote");
    } finally { preview.close(); }
    await chat.setViewport(1440, 900);
    await api.__testing.openPlanPreview(planPath);
    // VS Code may recreate the editor webview after maximizing the chat.
    const revisedPreview = await SettingsFrameDriver.connectFromEnvironment(".tc-plan-preview__body");
    try {
      await until(() => revisedPreview.evaluate<boolean>("document.body.innerText.includes('Test before implementation.')"), "preview refreshes the same plan file");
      await revisedPreview.capture(path.join(artifacts, "shared-plan-revised.png"));
    } finally { revisedPreview.close(); }
    assert.equal(api.__testing.getWebviewState().activeSessionId, sessionId);
    assert.equal(model.capturedNonTitleRequests().length, 10);
    await assert.rejects(api.__testing.getSessionState(foreign.sessionId), /unknown_session/);
    assert.equal(api.__testing.getObservedEvents().some(event => event.sessionId === foreign.sessionId), false);
    await api.__testing.sendWebviewIntent({ messageId: "observe-return-own", type: "switchSession", data: { sessionId: ownId } });
    await until(() => chat!.evaluate<boolean>("document.querySelector('[data-testid=\"composer-input\"]')?.getAttribute('contenteditable') === 'true'"), "own session becomes writable again");
    assert.deepEqual(api.__testing.getObservedWebviewErrors(), []);
    await fs.writeFile(path.join(artifacts, "observer-journey.json"), JSON.stringify({ sessionId, planId, planPath, ownId, foreignSessionId: foreign.sessionId, replacement, modelCalls: model.capturedNonTitleRequests().length, assertions: ["same-project disk list", "foreign project hidden and unreadable", "mid-stream prefix", "webview reload without duplication", "new ACP load/prompt visible once before first reply", "remote prompt preserves local draft", "real running and completed tool", "shared input busy and idle", "Send queues and Send now steers", "Stop cancels the real shell", "observer answers non-recommended question without task control", "quote and revise the same plan without execution", "own session writable"] }, null, 2));
  } finally {
    release();
    releaseResumedPrompt();
    await fs.writeFile(toolRelease, "cleanup");
    await fs.writeFile(cancelledRelease, "cleanup");
    chat?.close();
    try { await host.close(); } finally { await model.close(); }
  }
}
