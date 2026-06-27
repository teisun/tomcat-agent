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

const CHAT_E2E_SETTINGS = {
  "chat.allowAnonymousAccess": true,
  "chat.disableAIFeatures": false,
  "chat.mcp.discovery.enabled": false,
  "chat.mcp.enabled": false,
  "github.copilot.chat.githubMcpServer.enabled": false,
} as const;

export async function seedChatUserSettings(userDataDir: string): Promise<void> {
  const settingsDir = path.join(userDataDir, "User");
  await mkdir(settingsDir, { recursive: true });
  await writeFile(
    path.join(settingsDir, "settings.json"),
    `${JSON.stringify(CHAT_E2E_SETTINGS, null, 2)}\n`,
    "utf8",
  );
}

function buildFakeServeSource(editFilePath: string): string {
  const transcriptPlanMarkdown = JSON.stringify(`---
name: Transcript UI Showcase
overview: Review the transcript UI polish and confirm the merged plan card before building.
---

# Transcript UI showcase
`);
  return `#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");
const readline = require("node:readline");

const editFilePath = ${JSON.stringify(editFilePath)};
const MODEL_OPTIONS = ["fake-model", "gpt-5.4", "claude-4.6-sonnet"];
const sessions = new Map();
let sessionCounter = 1;
let historyCounter = 1;
let pendingApproval = null;
let pendingInterrupt = null;
let activeSessionId = null;
const transcriptProgressDelayMs = Math.max(
  0,
  Number(process.env.TOMCAT_E2E_TRANSCRIPT_PROGRESS_DELAY_MS || "250"),
);

function touchSession(session) {
  session.updatedAt = Date.now();
  return session;
}

function createSession() {
  const sessionId = \`session-\${sessionCounter++}\`;
  sessions.set(sessionId, touchSession({
    busy: false,
    cwd: process.cwd(),
    history: [],
    mode: "code",
    model: MODEL_OPTIONS[0],
    planId: null,
    planPath: null,
    planState: "chat",
    sessionKey: "fake-workspace",
  }));
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
    sessions.set(sessionId, touchSession({
      busy: false,
      cwd: process.cwd(),
      history: [],
      mode: "code",
      model: MODEL_OPTIONS[0],
      planId: null,
      planPath: null,
      planState: "chat",
      sessionKey: "fake-workspace",
    }));
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

function emitPlanEvent(sessionId, type) {
  const session = ensureSession(sessionId);
  send({
    path: session.planPath,
    planId: session.planId,
    sessionId,
    state: session.planState,
    type,
  });
}

function recordHistoryMessage(sessionId, role, content) {
  const session = touchSession(ensureSession(sessionId));
  session.history.push({
    id: \`h-\${historyCounter++}\`,
    message: {
      content,
      role,
    },
    type: "message",
  });
}

function emitContextMetrics(sessionId, ratio = 0.42) {
  send({
    compactionCount: 0,
    compactionTokensFreed: 0,
    contextUtilizationRatio: ratio,
    inputTokensUsed: 256,
    preheatInProgress: false,
    preheatResultPending: false,
    sessionId,
    totalToolResultBytesPersisted: 0,
    type: "context_metrics_update",
  });
}

function finishTurn(sessionId, error = null) {
  const session = touchSession(ensureSession(sessionId));
  session.busy = false;
  send({
    error,
    messages: [],
    sessionId,
    type: "agent_end",
  });
}

function startTurn(sessionId) {
  const session = touchSession(ensureSession(sessionId));
  session.busy = true;
  activeSessionId = sessionId;
  send({
    sessionId,
    type: "agent_start",
  });
}

function handlePrompt(frame) {
  const sessionId = frame.sessionId || activeSessionId || createSession();
  const session = touchSession(ensureSession(sessionId));
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
  const attachmentCount = Array.isArray(frame.params && frame.params.attachments)
    ? frame.params.attachments.length
    : 0;
  recordHistoryMessage(sessionId, "user", text);
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
      recordHistoryMessage(sessionId, "assistant", "late completion");
      emitContextMetrics(sessionId, 0.36);
      finishTurn(sessionId, null);
    }, 1000);
    return;
  }

  if (text.includes("transcript ui")) {
    const readPath = editFilePath;
    emitMessageDelta(sessionId, "I will read the file and refresh the plan.");
    send({
      assistantMessageEvent: {
        delta: "Deciding which file to inspect for the transcript UI showcase.",
        kind: "thinking_delta",
      },
      message: {},
      sessionId,
      type: "message_update",
    });
    send({
      args: { path: readPath },
      sessionId,
      toolCallId: "tc-transcript-read",
      toolName: "read",
      type: "tool_execution_start",
    });
    send({
      display: { file: readPath, kind: "file" },
      isError: false,
      result: "fake file contents for transcript ui showcase",
      sessionId,
      toolCallId: "tc-transcript-read",
      toolName: "read",
      type: "tool_execution_end",
    });
    send({
      args: { command: "git status --short" },
      sessionId,
      toolCallId: "tc-transcript-bash",
      toolName: "bash",
      type: "tool_execution_start",
    });
    send({
      isError: false,
      result: "M src/app.tsx\\n M README.md\\n?? plans/transcript-ui-showcase.plan.md",
      sessionId,
      toolCallId: "tc-transcript-bash",
      toolName: "bash",
      type: "tool_execution_end",
    });
    send({
      args: { query: "vscode chat thinking collapsible" },
      sessionId,
      toolCallId: "tc-transcript-web-search",
      toolName: "web_search",
      type: "tool_execution_start",
    });
    send({
      isError: false,
      result: "Found 3 results.\\n- vscode/chatThinkingContentPart.ts\\n- chatCollapsibleContentPart.ts\\n- chatToolInvocationPart.ts",
      sessionId,
      toolCallId: "tc-transcript-web-search",
      toolName: "web_search",
      type: "tool_execution_end",
    });
    const planPath = path.join(process.cwd(), "plans", "transcript-ui-showcase.plan.md");
    fs.mkdirSync(path.dirname(planPath), { recursive: true });
    fs.writeFileSync(planPath, ${transcriptPlanMarkdown}, "utf8");
    session.planId = "transcript-ui-showcase";
    session.planPath = planPath;
    session.planState = "planning";
    send({
      path: planPath,
      planId: session.planId,
      sessionId,
      state: session.planState,
      type: "plan.create",
    });
    const planTodos = [
      { id: "t1", content: "Read the file", status: "completed" },
      { id: "t2", content: "Render the transcript UI", status: "in_progress" },
      { id: "t3", content: "Verify the screenshot crop", status: "pending" },
      { id: "t4", content: "Review the merged plan card", status: "pending" },
    ];
    session.planTodos = planTodos;
    send({
      planId: session.planId,
      sessionId,
      todos: planTodos,
      type: "plan.todos",
    });
    send({
      sessionId,
      title: "Transcript UI Showcase",
      type: "session.title_updated",
    });
    send({
      message: {},
      summaryTitle: "Reviewed 1 file",
      toolResults: [],
      turnIndex: 1,
      type: "turn_end",
    });
    const finishTranscriptTurn = () => {
      emitContextMetrics(sessionId, 0.55);
      recordHistoryMessage(sessionId, "assistant", "I will read the file and refresh the plan.");
      finishTurn(sessionId, null);
    };
    if (transcriptProgressDelayMs > 0) {
      setTimeout(finishTranscriptTurn, transcriptProgressDelayMs);
    } else {
      finishTranscriptTurn();
    }
    return;
  }

  const responseText = attachmentCount
    ? \`hello from fake tomcat (\${attachmentCount} attachments)\`
    : "hello from fake tomcat";
  emitMessageDelta(sessionId, responseText);
  recordHistoryMessage(sessionId, "assistant", responseText);
  emitContextMetrics(sessionId, attachmentCount ? 0.58 : 0.42);
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
      recordHistoryMessage(sessionId, "assistant", "edit applied");
      emitContextMetrics(sessionId, 0.47);
      finishTurn(sessionId, null);
    }, 250);
  } else {
    emitMessageDelta(sessionId, "edit rejected");
    recordHistoryMessage(sessionId, "assistant", "edit rejected");
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
              "list_models",
              "set_model",
              "set_plan_mode",
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
      touchSession(ensureSession(activeSessionId));
      send({
        id: frame.id,
        payload: { activeSessionId },
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
            isCurrent: sessionId === activeSessionId,
            sessionId,
            updatedAt: session.updatedAt,
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
          planId: session.planId,
          planState: session.planState,
          planTodos: session.planTodos ?? [],
          sessionId,
          sessionKey: session.sessionKey,
          sessionTodos: session.sessionTodos ?? [],
        },
        sessionId,
        success: true,
        type: "response",
      });
      break;
    }
    case "get_messages": {
      const sessionId = frame.sessionId || activeSessionId;
      const session = ensureSession(sessionId);
      send({
        id: frame.id,
        payload: {
          header: {
            cwd: session.cwd,
            id: sessionId,
            type: "session",
            version: 3,
          },
          messages: session.history,
          sessionId,
          upToSeq: null,
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
        payload: { closed: true, sessionId: frame.sessionId },
        success: true,
        type: "response",
      });
      break;
    case "list_models":
      send({
        id: frame.id,
        payload: {
          models: MODEL_OPTIONS.map((id) => ({ id })),
        },
        sessionId: activeSessionId,
        success: true,
        type: "response",
      });
      break;
    case "set_model": {
      const sessionId = frame.sessionId || activeSessionId || createSession();
      const session = touchSession(ensureSession(sessionId));
      session.model = frame.model;
      send({
        id: frame.id,
        payload: { model: session.model, sessionId },
        sessionId,
        success: true,
        type: "response",
      });
      break;
    }
    case "set_plan_mode": {
      const sessionId = frame.sessionId || activeSessionId || createSession();
      const session = touchSession(ensureSession(sessionId));
      if (frame.action === "enter") {
        session.planState = "planning";
        session.planId = session.planId || "fake-plan";
        session.planPath = path.join(process.cwd(), "plans", \`\${session.planId}.plan.md\`);
        fs.mkdirSync(path.dirname(session.planPath), { recursive: true });
        fs.writeFileSync(session.planPath, "# Fake plan\\n\\n- Step 1\\n", "utf8");
        send({
          id: frame.id,
          payload: { planId: session.planId, planState: session.planState },
          sessionId,
          success: true,
          type: "response",
        });
        emitPlanEvent(sessionId, "plan.create");
        break;
      }
      if (frame.action === "exit") {
        session.planState = "chat";
        session.planId = null;
        send({
          id: frame.id,
          payload: { planId: null, planState: "chat" },
          sessionId,
          success: true,
          type: "response",
        });
        emitPlanEvent(sessionId, "plan.complete");
        break;
      }

      session.planId = frame.planId || session.planId || "fake-plan";
      session.planPath = session.planPath || path.join(process.cwd(), "plans", \`\${session.planId}.plan.md\`);
      fs.mkdirSync(path.dirname(session.planPath), { recursive: true });
      fs.writeFileSync(session.planPath, "# Fake plan\\n\\n- Step 1\\n- Build\\n", "utf8");
      session.planState = "executing";
      send({
        id: frame.id,
        payload: {
          planId: session.planId,
          planPath: session.planPath,
          planState: session.planState,
        },
        sessionId,
        success: true,
        type: "response",
      });
      emitPlanEvent(sessionId, "plan.build");
      setTimeout(() => {
        finishTurn(sessionId, null);
      }, 10);
      break;
    }
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
