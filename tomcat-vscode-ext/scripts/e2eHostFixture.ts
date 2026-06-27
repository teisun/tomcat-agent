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
const debugGetMessages = process.env.TOMCAT_E2E_DEBUG_GET_MESSAGES === "1";
const historyPageDelayMs = Math.max(
  0,
  Number(process.env.TOMCAT_E2E_HISTORY_PAGE_DELAY_MS || "120"),
);
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
    contextRatio: null,
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
      contextRatio: null,
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

function emitCompletedTool(sessionId, tool) {
  send({
    args: tool.args || {},
    sessionId,
    toolCallId: tool.toolCallId,
    toolName: tool.toolName,
    type: "tool_execution_start",
  });
  send({
    display: tool.display,
    isError: false,
    result: tool.result,
    sessionId,
    toolCallId: tool.toolCallId,
    toolName: tool.toolName,
    type: "tool_execution_end",
  });
}

function buildToolIconShowcaseTools() {
  const workspaceDir = process.cwd();
  const outputPath = path.join(workspaceDir, "output.txt");
  const sourcePath = path.join(workspaceDir, "src", "app.tsx");
  const showcasePlanPath = path.join(workspaceDir, "plans", "tool-icon-showcase.plan.md");
  return [
    {
      toolCallId: "tc-showcase-read",
      toolName: "read",
      args: { path: editFilePath },
      display: { file: editFilePath, kind: "file" },
      result: "Read edit-target.txt",
    },
    {
      toolCallId: "tc-showcase-load-skill",
      toolName: "load_skill",
      args: { name: "sdk" },
      result: "Loaded skill sdk",
    },
    {
      toolCallId: "tc-showcase-write",
      toolName: "write",
      args: { path: outputPath },
      display: { file: outputPath, kind: "file" },
      result: "Created output.txt",
    },
    {
      toolCallId: "tc-showcase-edit",
      toolName: "edit",
      args: { path: sourcePath },
      display: { file: sourcePath, kind: "file" },
      result: "Edited src/app.tsx",
    },
    {
      toolCallId: "tc-showcase-hashline-edit",
      toolName: "hashline_edit",
      args: { path: sourcePath },
      display: { file: sourcePath, kind: "file" },
      result: "Edited line anchors in src/app.tsx",
    },
    {
      toolCallId: "tc-showcase-bash",
      toolName: "bash",
      args: { command: "npm run test" },
      result: "Tests passed",
    },
    {
      toolCallId: "tc-showcase-task-output",
      toolName: "task_output",
      args: { task_id: "task-1" },
      result: "task-1 log tail",
    },
    {
      toolCallId: "tc-showcase-task-stop",
      toolName: "task_stop",
      args: { task_id: "task-1" },
      result: "Stopped task-1",
    },
    {
      toolCallId: "tc-showcase-task-list",
      toolName: "task_list",
      args: {},
      result: "task-1 running",
    },
    {
      toolCallId: "tc-showcase-list-dir",
      toolName: "list_dir",
      args: { path: workspaceDir },
      result: "plans\\nsrc\\nREADME.md",
    },
    {
      toolCallId: "tc-showcase-search-files",
      toolName: "search_files",
      args: { path: "src", pattern: "ToolRow" },
      result: "src/components/ToolRow.tsx:1",
    },
    {
      toolCallId: "tc-showcase-web-search",
      toolName: "web_search",
      args: { query: "codicon list tree" },
      result: "Found 2 results.\\n- codicon reference\\n- vscode icon docs",
    },
    {
      toolCallId: "tc-showcase-web-fetch",
      toolName: "web_fetch",
      args: { url: "https://example.com/icons" },
      result: "Fetched https://example.com/icons",
    },
    {
      toolCallId: "tc-showcase-config-get",
      toolName: "config_get",
      args: { key: "llm.default_model" },
      result: "gpt-5.4",
    },
    {
      toolCallId: "tc-showcase-config-set",
      toolName: "config_set",
      args: { key: "log.level", value: "debug" },
      result: "Updated log.level",
    },
    {
      toolCallId: "tc-showcase-create-plan",
      toolName: "create_plan",
      args: { goal: "Tool icon showcase" },
      result: "Created plan tool-icon-showcase",
    },
    {
      toolCallId: "tc-showcase-update-plan",
      toolName: "update_plan",
      args: { plan_id: "tool-icon-showcase" },
      display: { kind: "plan", plan: showcasePlanPath },
      result: "Updated plan tool-icon-showcase",
    },
    {
      toolCallId: "tc-showcase-todos",
      toolName: "todos",
      args: { upsert: [{ id: "todo-1", content: "Review icons", status: "in_progress" }] },
      result: "Updated todos",
    },
    {
      toolCallId: "tc-showcase-ask-question",
      toolName: "ask_question",
      args: { questions: [{ id: "q1", prompt: "Ship the icon set?" }] },
      result: "Asked question",
    },
  ];
}

function buildGiantHistoryTools() {
  const workspaceDir = process.cwd();
  const sourcePath = path.join(workspaceDir, "src", "app.tsx");
  return Array.from({ length: 100 }, (_, index) => {
    const batch = index + 1;
    switch (index % 4) {
      case 0:
        return {
          toolCallId: \`tc-giant-read-\${batch}\`,
          toolName: "read",
          args: { path: sourcePath },
          display: { file: sourcePath, kind: "file" },
          result: \`Read src/app.tsx batch \${batch}\`,
        };
      case 1:
        return {
          toolCallId: \`tc-giant-bash-\${batch}\`,
          toolName: "bash",
          args: { command: \`echo batch-\${batch}\` },
          result: \`Ran batch \${batch}\`,
        };
      case 2:
        return {
          toolCallId: \`tc-giant-search-\${batch}\`,
          toolName: "web_search",
          args: { query: \`batch \${batch} history loading\` },
          result: \`Found history batch \${batch}\`,
        };
      default:
        return {
          toolCallId: \`tc-giant-list-\${batch}\`,
          toolName: "list_dir",
          args: { path: workspaceDir },
          result: \`Listed workspace batch \${batch}\`,
        };
    }
  });
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
  const payload = {
    path: session.planPath,
    planId: session.planId,
    sessionId,
    state: session.planState,
    type,
  };
  send(payload);
  session.history.push({
    event: type,
    id: \`h-\${historyCounter++}\`,
    path: session.planPath,
    plan_id: session.planId,
    state: session.planState,
    type: "custom",
  });
}

function emitCustomPlanEvent(sessionId, type, extra = {}) {
  const session = ensureSession(sessionId);
  const pathValue = Object.prototype.hasOwnProperty.call(extra, "path")
    ? extra.path
    : session.planPath;
  const planIdValue = Object.prototype.hasOwnProperty.call(extra, "planId")
    ? extra.planId
    : session.planId;
  const stateValue = Object.prototype.hasOwnProperty.call(extra, "state")
    ? extra.state
    : session.planState;
  const historyExtra = { ...extra };
  delete historyExtra.planId;
  send({
    ...extra,
    path: pathValue,
    planId: planIdValue,
    sessionId,
    state: stateValue,
    type,
  });
  session.history.push({
    ...historyExtra,
    event: type,
    id: \`h-\${historyCounter++}\`,
    path: pathValue,
    plan_id: planIdValue,
    state: stateValue,
    type: "custom",
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

function recordHistoryAssistantWithTools(sessionId, content, tools, summaryTitle) {
  const session = touchSession(ensureSession(sessionId));
  session.history.push({
    id: \`h-\${historyCounter++}\`,
    message: {
      content,
      role: "assistant",
      summary_title: summaryTitle || undefined,
      tool_calls: tools.map((tool) => ({
        function: {
          arguments: JSON.stringify(tool.args || {}),
          name: tool.toolName,
        },
        id: tool.toolCallId,
        type: "function",
      })),
    },
    type: "message",
  });
}

function recordHistoryToolResult(sessionId, tool) {
  const session = touchSession(ensureSession(sessionId));
  session.history.push({
    id: \`h-\${historyCounter++}\`,
    message: {
      content: tool.result,
      role: "tool",
      tool_call_id: tool.toolCallId,
    },
    type: "message",
  });
}

function emitContextMetrics(sessionId, ratio = 0.42) {
  const session = touchSession(ensureSession(sessionId));
  session.contextRatio = ratio;
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

  if (text.includes("tool icon showcase")) {
    emitMessageDelta(sessionId, "I prepared a built-in tool icon showcase.");
    send({
      assistantMessageEvent: {
        delta: "Review every built-in tool row and icon in one place.",
        kind: "thinking_delta",
      },
      message: {},
      sessionId,
      type: "message_update",
    });
    const showcaseTools = buildToolIconShowcaseTools();
    for (const tool of showcaseTools) {
      emitCompletedTool(sessionId, tool);
    }
    send({
      message: {},
      summaryTitle: "Built-in tool icons",
      toolResults: [],
      turnIndex: 1,
      type: "turn_end",
    });
    emitContextMetrics(sessionId, 0.31);
    recordHistoryAssistantWithTools(
      sessionId,
      "I prepared a built-in tool icon showcase.",
      showcaseTools,
      "Built-in tool icons",
    );
    for (const tool of showcaseTools) {
      recordHistoryToolResult(sessionId, tool);
    }
    finishTurn(sessionId, null);
    return;
  }

  if (text.includes("giant tool history")) {
    emitMessageDelta(sessionId, "I prepared a giant historical tool group.");
    send({
      assistantMessageEvent: {
        delta: "Keep loading older pages until the whole group is ready.",
        kind: "thinking_delta",
      },
      message: {},
      sessionId,
      type: "message_update",
    });
    const giantTools = buildGiantHistoryTools();
    for (const tool of giantTools) {
      emitCompletedTool(sessionId, tool);
    }
    send({
      message: {},
      summaryTitle: "Giant history tool group",
      toolResults: [],
      turnIndex: 1,
      type: "turn_end",
    });
    emitContextMetrics(sessionId, 0.34);
    recordHistoryAssistantWithTools(
      sessionId,
      "I prepared a giant historical tool group.",
      giantTools,
      "Giant history tool group",
    );
    for (const tool of giantTools) {
      recordHistoryToolResult(sessionId, tool);
    }
    finishTurn(sessionId, null);
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
    emitPlanEvent(sessionId, "plan.create");
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

  if (text.includes("plan replay")) {
    session.planId = "history-plan";
    session.planPath = path.join(process.cwd(), "plans", "history-plan.plan.md");
    fs.mkdirSync(path.dirname(session.planPath), { recursive: true });
    fs.writeFileSync(session.planPath, "# History plan\\n\\n- Review\\n- Verify\\n", "utf8");
    session.planState = "pending";
    emitPlanEvent(sessionId, "plan.create");
    emitPlanEvent(sessionId, "plan.build");
    emitCustomPlanEvent(sessionId, "plan.review", { summary: "looks good" });
    emitCustomPlanEvent(sessionId, "plan.verify", { verdict: "pass" });
    emitPlanEvent(sessionId, "plan.pending");
    emitMessageDelta(sessionId, "I replayed the plan review and verify history.");
    recordHistoryMessage(
      sessionId,
      "assistant",
      "I replayed the plan review and verify history.",
    );
    emitContextMetrics(sessionId, 0.62);
    finishTurn(sessionId, null);
    return;
  }

  if (text.includes("cross owner plan")) {
    session.planId = "participant-plan";
    session.planPath = path.join(process.cwd(), "plans", "participant-plan.plan.md");
    fs.mkdirSync(path.dirname(session.planPath), { recursive: true });
    fs.writeFileSync(session.planPath, "# Participant plan\\n\\n- Enter\\n- Build\\n- Exit\\n", "utf8");
    session.planState = "planning";
    emitPlanEvent(sessionId, "plan.enter");
    setTimeout(() => {
      session.planState = "executing";
      emitPlanEvent(sessionId, "plan.build");
    }, 1000);
    setTimeout(() => {
      const lastPlanId = session.planId;
      const lastPlanPath = session.planPath;
      session.planState = "chat";
      emitCustomPlanEvent(sessionId, "plan.exit", {
        path: lastPlanPath,
        planId: lastPlanId,
        state: "chat",
      });
      session.planId = null;
      session.planPath = null;
      recordHistoryMessage(sessionId, "assistant", "participant plan lifecycle finished");
      finishTurn(sessionId, null);
    }, 2000);
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
          contextUtilizationRatio: session.contextRatio,
          cwd: session.cwd,
          mode: session.mode,
          model: session.model,
          planId: session.planId,
          planPath: session.planPath,
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
      const rawLimit =
        frame.params && Object.prototype.hasOwnProperty.call(frame.params, "limit")
          ? frame.params.limit
          : frame.limit;
      const parsedLimit =
        typeof rawLimit === "number"
          ? rawLimit
          : Number.parseInt(String(rawLimit ?? ""), 10);
      const requestedLimit = parsedLimit > 0 ? parsedLimit : session.history.length;
      const rawCursor =
        frame.params && Object.prototype.hasOwnProperty.call(frame.params, "cursor")
          ? frame.params.cursor
          : frame.cursor;
      const requestedCursor =
        typeof rawCursor === "string" || typeof rawCursor === "number"
          ? Number.parseInt(String(rawCursor), 10)
          : NaN;
      const endExclusive =
        Number.isInteger(requestedCursor) && requestedCursor > 0
          ? Math.min(requestedCursor, session.history.length)
          : session.history.length;
      const start = Math.max(0, endExclusive - requestedLimit);
      const messages = session.history.slice(start, endExclusive);
      const hasMore = start > 0;
      if (debugGetMessages) {
        console.error(
          "[fake-serve:get_messages]",
          JSON.stringify({
            endExclusive,
            hasMore,
            messageCount: messages.length,
            rawCursor,
            rawLimit,
            requestedCursor,
            requestedLimit,
            sessionId,
            start,
            total: session.history.length,
          }),
        );
      }
      const response = {
        id: frame.id,
        payload: {
          header: {
            cwd: session.cwd,
            id: sessionId,
            type: "session",
            version: 3,
          },
          hasMore,
          messages,
          nextCursor: hasMore ? String(start) : null,
          sessionId,
          upToSeq: null,
        },
        sessionId,
        success: true,
        type: "response",
      };
      if (Number.isInteger(requestedCursor) && requestedCursor > 0 && historyPageDelayMs > 0) {
        setTimeout(() => send(response), historyPageDelayMs);
      } else {
        send(response);
      }
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
        emitPlanEvent(sessionId, "plan.enter");
        break;
      }
      if (frame.action === "exit") {
        const lastPlanId = session.planId;
        const lastPlanPath = session.planPath;
        session.planState = "chat";
        send({
          id: frame.id,
          payload: { planId: null, planState: "chat" },
          sessionId,
          success: true,
          type: "response",
        });
        emitCustomPlanEvent(sessionId, "plan.exit", {
          path: lastPlanPath,
          planId: lastPlanId,
          state: "chat",
        });
        session.planId = null;
        session.planPath = null;
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
