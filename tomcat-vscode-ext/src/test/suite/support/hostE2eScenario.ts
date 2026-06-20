import * as assert from "node:assert/strict";
import * as fs from "node:fs/promises";

import * as vscode from "vscode";

import type { TomcatExtensionApi } from "../../../extension";

function requireEnv(name: string): string {
  const value = process.env[name];
  assert.ok(value, `expected ${name} to be defined for host E2E`);
  return value;
}

function collectStreamText(
  stream: Awaited<ReturnType<TomcatExtensionApi["__testing"]["runParticipantTurn"]>>["stream"],
  kind: "markdown" | "progress",
): string {
  return stream
    .flatMap((event) =>
      event.kind === kind ? [event.value] : [],
    )
    .join("\n");
}

function getButton(
  stream: Awaited<ReturnType<TomcatExtensionApi["__testing"]["runParticipantTurn"]>>["stream"],
  title: string,
): Extract<
  Awaited<ReturnType<TomcatExtensionApi["__testing"]["runParticipantTurn"]>>["stream"][number],
  { kind: "button" }
> {
  const button = stream.find(
    (event): event is Extract<(typeof stream)[number], { kind: "button" }> =>
      event.kind === "button" && event.title === title,
  );
  assert.ok(button, `expected stream button ${title}`);
  return button;
}

export async function getTomcatExtensionApi(): Promise<TomcatExtensionApi> {
  const extension = vscode.extensions.getExtension<TomcatExtensionApi>(
    "tomcat.tomcat-vscode-ext",
  );

  assert.ok(extension, "expected Tomcat extension to be discoverable");
  const exports = await extension.activate();
  assert.ok(extension.isActive, "expected Tomcat extension to activate");
  return exports;
}

export async function assertParticipantHappyPath(
  api: TomcatExtensionApi,
): Promise<void> {
  const turn = await api.__testing.runParticipantTurn({
    prompt: "hello fake tomcat",
  });
  const markdown = collectStreamText(turn.stream, "markdown");

  assert.match(markdown, /hello from fake tomcat/i);
  assert.equal(typeof turn.result?.metadata?.sessionId, "string");
}

export async function assertApprovalDiffFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  const editFile = requireEnv("TOMCAT_VSCODE_TEST_EDIT_FILE");
  await fs.writeFile(editFile, "before\n", "utf8");

  const turn = await api.__testing.runParticipantTurn({
    autoClickTitles: ["Approve"],
    prompt: "approve edit",
  });

  assert.match(collectStreamText(turn.stream, "markdown"), /edit applied/i);
  getButton(turn.stream, "Open Diff");
  const applyButton = getButton(turn.stream, "Apply Edit");
  const toolCallId = (applyButton.arguments?.[0] as { toolCallId?: string } | undefined)
    ?.toolCallId;

  assert.ok(toolCallId, "expected diff/apply button to carry toolCallId");
  const prepared = api.__testing.getPreparedChange(toolCallId);
  assert.ok(prepared, "expected prepared change");
  assert.equal(prepared.originalContent, "before\n");
  assert.equal(prepared.proposedContent, "after\n");

  await api.__testing.openPreparedDiff(toolCallId);
  assert.equal(await api.__testing.applyPreparedEdit(toolCallId), true);
  assert.equal(await fs.readFile(editFile, "utf8"), "after\n");

  await vscode.commands.executeCommand("workbench.action.closeAllEditors");
}

export async function assertInterruptAndRestartFlow(
  api: TomcatExtensionApi,
): Promise<void> {
  const interrupted = await api.__testing.runParticipantTurn({
    cancelAfterMs: 50,
    prompt: "interrupt please",
  });
  assert.match(
    collectStreamText(interrupted.stream, "progress"),
    /interrupted/i,
  );

  const beforeRestartSessions = await api.__testing.listSessions();
  assert.ok(
    beforeRestartSessions.sessions.length >= 1,
    "expected at least one session before restart",
  );

  await api.__testing.restartServe();

  const afterRestart = await api.__testing.runParticipantTurn({
    prompt: "hello after restart",
  });
  assert.match(
    collectStreamText(afterRestart.stream, "markdown"),
    /hello from fake tomcat/i,
  );
}

export async function assertMultiSessionRouting(
  api: TomcatExtensionApi,
): Promise<void> {
  const sessionA = await api.__testing.runParticipantTurn({
    prompt: "thread A",
  });
  const sessionAId = sessionA.result?.metadata?.sessionId;
  assert.equal(typeof sessionAId, "string");

  const sessionB = await api.__testing.runParticipantTurn({
    prompt: "thread B",
  });
  const sessionBId = sessionB.result?.metadata?.sessionId;
  assert.equal(typeof sessionBId, "string");
  assert.notEqual(sessionAId, sessionBId);

  const followUpA = await api.__testing.runParticipantTurn({
    historySessionId: sessionAId,
    prompt: "follow up A",
  });
  const followUpB = await api.__testing.runParticipantTurn({
    historySessionId: sessionBId,
    prompt: "follow up B",
  });

  assert.equal(followUpA.result?.metadata?.sessionId, sessionAId);
  assert.equal(followUpB.result?.metadata?.sessionId, sessionBId);

  const sessions = await api.__testing.listSessions();
  assert.ok(
    sessions.sessions.some((session) => session.sessionId === sessionAId),
    "expected session A to remain listed",
  );
  assert.ok(
    sessions.sessions.some((session) => session.sessionId === sessionBId),
    "expected session B to remain listed",
  );
}
