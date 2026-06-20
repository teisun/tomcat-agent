import * as os from "node:os";
import * as path from "node:path";
import { chmod, mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";

export interface HostE2eFixture {
  cleanup(): Promise<void>;
  editFilePath: string;
  env: NodeJS.ProcessEnv;
  fakeServePath: string;
  rootDir: string;
  workspaceDir: string;
}

function buildFakeServeSource(editFilePath: string): string {
  return `#!/usr/bin/env node
const fs = require("node:fs");
const readline = require("node:readline");

const editFilePath = ${JSON.stringify(editFilePath)};
const sessions = new Map();
let sessionCounter = 1;
let pendingApproval = null;
let pendingInterrupt = null;
let activeSessionId = null;

function createSession() {
  const sessionId = \`session-\${sessionCounter++}\`;
  sessions.set(sessionId, { busy: false, cwd: process.cwd(), mode: "code", model: "fake-model" });
  if (!activeSessionId) {
    activeSessionId = sessionId;
  }
  return sessionId;
}

createSession();

function send(frame) {
  process.stdout.write(JSON.stringify(frame) + "\\n");
}

function ensureSession(sessionId) {
  if (!sessions.has(sessionId)) {
    sessions.set(sessionId, { busy: false, cwd: process.cwd(), mode: "code", model: "fake-model" });
  }
  return sessions.get(sessionId);
}

function emitMessageDelta(sessionId, delta) {
  send({
    assistantMessageEvent: {
      delta,
      kind: "content_delta",
    },
    message: {},
    sessionId,
    type: "message_update",
  });
}

function finishTurn(sessionId, error = null) {
  const session = ensureSession(sessionId);
  session.busy = false;
  send({
    error,
    messages: [],
    sessionId,
    type: "agent_end",
  });
}

function startTurn(sessionId) {
  const session = ensureSession(sessionId);
  session.busy = true;
  activeSessionId = sessionId;
  send({
    sessionId,
    type: "agent_start",
  });
}

function handlePrompt(frame) {
  const sessionId = frame.sessionId || activeSessionId || createSession();
  const session = ensureSession(sessionId);
  if (session.busy) {
    send({
      error: "busy",
      id: frame.id,
      payload: { busy: true },
      sessionId,
      success: false,
      type: "response",
    });
    return;
  }

  send({
    id: frame.id,
    payload: { accepted: true },
    sessionId,
    success: true,
    type: "response",
  });
  startTurn(sessionId);

  const text = String(frame.text || "");
  if (text.includes("approve edit")) {
    const requestId = \`ask-\${sessionId}\`;
    pendingApproval = { requestId, sessionId };
    send({
      payload: {
        questions: [
          {
            id: "approve-edit",
            options: [
              { id: "approve", label: "Approve", recommended: true },
              { id: "reject", label: "Reject", recommended: false },
            ],
            prompt: "Approve the edit?",
          },
        ],
        requestId,
        responseEvent: \`plan.ask_question.response.\${requestId}\`,
      },
      requestId,
      sessionId,
      subtype: "ask_question",
      type: "control_request",
    });
    return;
  }

  if (text.includes("interrupt")) {
    emitMessageDelta(sessionId, "partial");
    pendingInterrupt = setTimeout(() => {
      pendingInterrupt = null;
      emitMessageDelta(sessionId, "late completion");
      finishTurn(sessionId, null);
    }, 1000);
    return;
  }

  emitMessageDelta(sessionId, "hello from fake tomcat");
  finishTurn(sessionId, null);
}

function handleControlResponse(frame) {
  if (!pendingApproval || frame.requestId !== pendingApproval.requestId) {
    return;
  }

  const sessionId = pendingApproval.sessionId;
  const answers = frame.payload && frame.payload.result && Array.isArray(frame.payload.result.answers)
    ? frame.payload.result.answers
    : [];
  const firstAnswer = answers[0] || {};
  const pickedApprove = Array.isArray(firstAnswer.optionIds) && firstAnswer.optionIds.includes("approve");

  if (pickedApprove) {
    send({
      args: { path: editFilePath },
      sessionId,
      toolCallId: "tool-edit-1",
      toolName: "write",
      type: "tool_execution_start",
    });
    setTimeout(() => {
      fs.writeFileSync(editFilePath, "after\\n", "utf8");
      send({
        display: { file: editFilePath, kind: "file" },
        isError: false,
        result: { ok: true },
        sessionId,
        toolCallId: "tool-edit-1",
        toolName: "write",
        type: "tool_execution_end",
      });
      emitMessageDelta(sessionId, "edit applied");
      finishTurn(sessionId, null);
    }, 250);
  } else {
    emitMessageDelta(sessionId, "edit rejected");
    finishTurn(sessionId, null);
  }

  pendingApproval = null;
}

function handleInterrupt(frame) {
  const sessionId = frame.sessionId || activeSessionId;
  send({
    id: frame.id,
    payload: { interrupted: true },
    sessionId,
    success: true,
    type: "response",
  });

  if (!sessionId) {
    return;
  }

  if (pendingInterrupt) {
    clearTimeout(pendingInterrupt);
    pendingInterrupt = null;
  }

  const session = ensureSession(sessionId);
  if (session.busy) {
    send({
      sessionId,
      type: "agent_interrupted",
    });
    finishTurn(sessionId, "interrupted");
  }
}

function handleCommand(frame) {
  switch (frame.type) {
    case "control_request":
      if (frame.subtype === "initialize") {
        const sessionId = activeSessionId || createSession();
        send({
          payload: {
            capabilities: [
              "prompt",
              "ask_question",
              "new_session",
              "switch_session",
              "list_sessions",
              "get_state",
              "close_session",
              "interrupt",
              "follow_up",
            ],
            protocolVersion: 1,
            sessionId,
          },
          requestId: frame.requestId,
          sessionId,
          type: "control_response",
        });
      }
      break;
    case "control_response":
      handleControlResponse(frame);
      break;
    case "new_session": {
      const sessionId = createSession();
      activeSessionId = sessionId;
      send({
        id: frame.id,
        payload: { sessionId },
        sessionId,
        success: true,
        type: "response",
      });
      break;
    }
    case "switch_session": {
      activeSessionId = frame.sessionId;
      ensureSession(activeSessionId);
      send({
        id: frame.id,
        payload: { sessionId: activeSessionId },
        sessionId: activeSessionId,
        success: true,
        type: "response",
      });
      break;
    }
    case "list_sessions":
      send({
        id: frame.id,
        payload: {
          activeSessionId,
          sessions: [...sessions.entries()].map(([sessionId, session]) => ({
            busy: session.busy,
            sessionId,
          })),
        },
        success: true,
        type: "response",
      });
      break;
    case "get_state": {
      const sessionId = frame.sessionId || activeSessionId;
      const session = ensureSession(sessionId);
      send({
        id: frame.id,
        payload: {
          busy: session.busy,
          cwd: session.cwd,
          mode: session.mode,
          model: session.model,
          sessionId,
        },
        sessionId,
        success: true,
        type: "response",
      });
      break;
    }
    case "close_session":
      sessions.delete(frame.sessionId);
      if (activeSessionId === frame.sessionId) {
        activeSessionId = sessions.keys().next().value || null;
      }
      send({
        id: frame.id,
        payload: { closed: true },
        success: true,
        type: "response",
      });
      break;
    case "prompt":
    case "follow_up":
      handlePrompt(frame);
      break;
    case "interrupt":
      handleInterrupt(frame);
      break;
    default:
      send({
        error: \`unknown_command: \${frame.type}\`,
        id: frame.id || null,
        success: false,
        type: "response",
      });
      break;
  }
}

const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
rl.on("line", (line) => {
  if (!line.trim()) {
    return;
  }

  let parsed;
  try {
    parsed = JSON.parse(line);
  } catch (error) {
    send({
      error: \`parse_error: \${String(error && error.message || error)}\`,
      success: false,
      type: "response",
    });
    return;
  }

  handleCommand(parsed);
});
`;
}

export async function createHostE2eFixture(): Promise<HostE2eFixture> {
  const rootDir = await mkdtemp(path.join(os.tmpdir(), "tomcat-vscode-ext-host-"));
  const workspaceDir = path.join(rootDir, "workspace");
  const fakeServePath = path.join(rootDir, "fake-tomcat.js");
  const editFilePath = path.join(rootDir, "edit-target.txt");

  await mkdir(workspaceDir, { recursive: true });
  await writeFile(editFilePath, "before\n", "utf8");
  await writeFile(fakeServePath, buildFakeServeSource(editFilePath), "utf8");
  await chmod(fakeServePath, 0o755);

  return {
    async cleanup() {
      await rm(rootDir, { force: true, recursive: true });
    },
    editFilePath,
    env: {
      ...process.env,
      TOMCAT_VSCODE_TEST_DEFAULT_CWD: workspaceDir,
      TOMCAT_VSCODE_TEST_EDIT_FILE: editFilePath,
      TOMCAT_VSCODE_TEST_PATH: fakeServePath,
      TOMCAT_VSCODE_TEST_SUPPRESS_EXIT_PROMPT: "1",
    },
    fakeServePath,
    rootDir,
    workspaceDir,
  };
}

export function resolveVsCodeCli(): string {
  return (
    process.env.VSCODE_CLI_PATH ||
    "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code"
  );
}

export function resolveVsCodeExecutable(): string {
  return (
    process.env.VSCODE_EXECUTABLE_PATH ||
    "/Applications/Visual Studio Code.app/Contents/MacOS/Electron"
  );
}

export function resolveCursorCli(): string {
  return process.env.CURSOR_CLI_PATH || "cursor";
}

export function resolveCursorExecutable(): string {
  return (
    process.env.CURSOR_EXECUTABLE_PATH ||
    "/Applications/Cursor.app/Contents/MacOS/Cursor"
  );
}
