#!/usr/bin/env node

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const readline = require("node:readline");

const MODEL_OPTIONS = [
  "gpt-5.4",
  "claude-4.6-sonnet",
  "deepseek-v4-pro",
  "manual-acceptance-model",
];

const LONG_SENTENCE =
  "Tomcat webview manual acceptance fixture keeps this response intentionally verbose so the real VSCode sidebar has enough vertical content to exercise auto-scroll, sticky bottom behavior, and tool-card overflow handling without depending on a live model.";

const TOOL_RESULT_BLOCK = [
  "match: gui/src/App.tsx -> wired useAutoScroll into the transcript shell",
  "match: gui/src/components/ThinkingBlock.tsx -> collapse/expand affordance preserved while streaming",
  "match: gui/src/components/ToolCallCard.tsx -> complete cards collapse by default, errors stay open",
  "match: gui/src/styles.css -> composer bar uses a single nowrap row with model as the flex item",
  "match: src/ui/webview/state.ts -> historical thinking and role:tool hydration remain stable",
].join("\n");

const ERROR_TEXT = [
  "Validation failed for acceptance probe:",
  "- simulated stderr line 1: width clamp remained within expected bounds",
  "- simulated stderr line 2: screenshot review intentionally surfaced a warning card",
  "- simulated stderr line 3: this is a fake error used to verify expanded error presentation",
].join("\n");

const STATE_DIR = process.env.TOMCAT_FAKE_SERVE_STATE_DIR || os.tmpdir();
const STATE_PATH = path.join(STATE_DIR, "manual-acceptance-fake-serve-state.json");

const timers = new Set();
const sessions = new Map();
const pendingApprovals = new Map();

let activeSessionId = null;
let askQuestionCounter = 1;
let assistantMessageCounter = 1;
let historyCounter = 1;
let sessionCounter = 1;

function defaultThinkingLevels() {
  return {
    "claude-4.6-sonnet": "medium",
    "deepseek-v4-pro": "low",
    "gpt-5.4": "high",
    "manual-acceptance-model": "xhigh",
  };
}

function normalizeThinkingLevels(value) {
  return {
    ...defaultThinkingLevels(),
    ...(value && typeof value === "object" ? value : {}),
  };
}

function currentThinkingLevel(session) {
  return session.modelThinking?.[session.model] || null;
}

function deriveTitleFromHistory(history) {
  const firstUser = history.find((entry) => entry?.type === "message" && entry?.message?.role === "user");
  const text = firstUser?.message?.content ?? "";
  const firstLine = String(text)
    .split("\n")
    .map((line) => line.trim())
    .find(Boolean);
  if (!firstLine) {
    return null;
  }
  const chars = [...firstLine];
  return chars.length > 40 ? `${chars.slice(0, 40).join("")}\u{2026}` : firstLine;
}

function normalizeSession(session) {
  const history = Array.isArray(session?.history) ? session.history : buildSeedHistory();
  return {
    busy: false,
    cwd: typeof session?.cwd === "string" ? session.cwd : process.cwd(),
    history,
    mode: typeof session?.mode === "string" ? session.mode : "code",
    model:
      typeof session?.model === "string" && MODEL_OPTIONS.includes(session.model)
        ? session.model
        : MODEL_OPTIONS[0],
    modelThinking: normalizeThinkingLevels(session?.modelThinking),
    pendingAssistantMessageId: session?.pendingAssistantMessageId ?? null,
    planId: session?.planId ?? null,
    planPath: session?.planPath ?? null,
    planState: typeof session?.planState === "string" ? session.planState : "chat",
    sessionKey:
      typeof session?.sessionKey === "string"
        ? session.sessionKey
        : "manual-acceptance-workspace",
    title: typeof session?.title === "string" && session.title.length > 0
      ? session.title
      : deriveTitleFromHistory(history),
    updatedAt: typeof session?.updatedAt === "number" ? session.updatedAt : Date.now(),
  };
}

function persistState() {
  try {
    fs.mkdirSync(STATE_DIR, { recursive: true });
    fs.writeFileSync(
      STATE_PATH,
      `${JSON.stringify(
        {
          activeSessionId,
          askQuestionCounter,
          historyCounter,
          sessionCounter,
          sessions: Object.fromEntries(sessions.entries()),
        },
        null,
        2,
      )}\n`,
      "utf8",
    );
  } catch (error) {
    console.error(`[fake-serve] failed to persist state: ${String((error && error.message) || error)}`);
  }
}

function loadState() {
  if (!fs.existsSync(STATE_PATH)) {
    return;
  }
  try {
    const parsed = JSON.parse(fs.readFileSync(STATE_PATH, "utf8"));
    activeSessionId =
      typeof parsed.activeSessionId === "string" || parsed.activeSessionId === null
        ? parsed.activeSessionId
        : null;
    askQuestionCounter =
      typeof parsed.askQuestionCounter === "number" ? parsed.askQuestionCounter : askQuestionCounter;
    historyCounter =
      typeof parsed.historyCounter === "number" ? parsed.historyCounter : historyCounter;
    sessionCounter =
      typeof parsed.sessionCounter === "number" ? parsed.sessionCounter : sessionCounter;
    sessions.clear();
    for (const [sessionId, session] of Object.entries(parsed.sessions || {})) {
      sessions.set(sessionId, normalizeSession(session));
    }
    if (!activeSessionId || !sessions.has(activeSessionId)) {
      activeSessionId = sessions.keys().next().value || null;
    }
  } catch (error) {
    console.error(`[fake-serve] failed to load state: ${String((error && error.message) || error)}`);
  }
}

const ASK_QUESTION_REVERIFY_PATTERN = /ask question reverify/i;
const ASK_QUESTION_PROMPTS = [
  {
    id: "q1",
    options: [
      { id: "day", label: "白天", recommended: true },
      { id: "night", label: "晚上" },
    ],
    prompt: "你更喜欢在什么时候写代码?",
  },
  {
    id: "q2",
    options: [
      { id: "ts", label: "TypeScript", recommended: true },
      { id: "rs", label: "Rust" },
    ],
    prompt: "你现在更想写哪种语言?",
  },
];

function send(frame) {
  process.stdout.write(`${JSON.stringify(frame)}\n`);
}

function createHistoryEntry(message) {
  return {
    id: `hist-${historyCounter++}`,
    message,
    type: "message",
  };
}

function recordHistoryEntry(session, message) {
  const entry =
    message && message.role === "assistant"
      ? {
          id: ensurePendingAssistantMessageId(session),
          message,
          type: "message",
        }
      : createHistoryEntry(message);
  session.history.push(entry);
  persistState();
  return entry;
}

function makeParagraph(index) {
  return `${LONG_SENTENCE} Historic paragraph ${index} also references timeline ordering, session hydration, and responsive controls so the transcript grows tall enough for scroll verification.`;
}

function buildSeedHistory() {
  const history = [];

  for (let index = 1; index <= 5; index += 1) {
    history.push(
      createHistoryEntry({
        content: `Historic prompt ${index}: verify transcript turn ${index} in the Tomcat sidebar.`,
        role: "user",
      }),
    );
    history.push(
      createHistoryEntry({
        content: `${makeParagraph(index)}\n\n${makeParagraph(index + 20)}`,
        role: "assistant",
      }),
    );
  }

  history.push(
    createHistoryEntry({
      content: "Historic prompt 6: confirm thinking appears before the assistant reply and tool output stays collapsible.",
      role: "user",
    }),
  );
  history.push(
    createHistoryEntry({
      content: `${makeParagraph(6)}\n\nThe assistant answer intentionally lands after the thinking block so the webview can prove chronological rendering.`,
      role: "assistant",
      thinking_text:
        "Historic thinking block: inspect the session timeline first, then group any tool output under the matching assistant turn so reload hydration keeps the same order users saw while streaming.",
      tool_calls: [
        {
          function: { name: "search_files" },
          id: "hist-tool-search-1",
        },
      ],
    }),
  );
  history.push(
    createHistoryEntry({
      content: `${TOOL_RESULT_BLOCK}\n\nsummary: historical tool output is deliberately long so the expanded card needs its own internal scrollbar.`,
      role: "tool",
      tool_call_id: "hist-tool-search-1",
    }),
  );

  history.push(
    createHistoryEntry({
      content: "Historic prompt 7: confirm the composer keeps one line when the sidebar becomes narrow.",
      role: "user",
    }),
  );
  history.push(
    createHistoryEntry({
      content: `${makeParagraph(7)}\n\nFallback reasoning text is used here to cover both extraction paths.`,
      reasoning_continuation: {
        fallback_text:
          "Historic fallback reasoning: shrink the model selector first, keep mode, context, add, and send controls pinned, and only hide labels at the smallest widths.",
      },
      role: "assistant",
    }),
  );

  return history;
}

function touchSession(session) {
  session.updatedAt = Date.now();
  return session;
}

function createSession() {
  const sessionId = `manual-session-${sessionCounter++}`;
  const session = touchSession(
    normalizeSession({
      history: buildSeedHistory(),
    }),
  );
  sessions.set(sessionId, session);
  activeSessionId = activeSessionId ?? sessionId;
  persistState();
  return sessionId;
}

function ensureSession(sessionId) {
  if (!sessionId || !sessions.has(sessionId)) {
    const createdId = createSession();
    return sessions.get(createdId);
  }
  return sessions.get(sessionId);
}

function nextAssistantMessageId() {
  return `assistant-${assistantMessageCounter++}`;
}

function ensurePendingAssistantMessageId(session) {
  if (!session.pendingAssistantMessageId) {
    session.pendingAssistantMessageId = nextAssistantMessageId();
  }
  return session.pendingAssistantMessageId;
}

function clearPendingAssistantMessageId(session) {
  session.pendingAssistantMessageId = null;
}

function emitMessageDelta(sessionId, kind, delta) {
  const session = touchSession(ensureSession(sessionId));
  send({
    assistantMessageId: ensurePendingAssistantMessageId(session),
    assistantMessageEvent: {
      delta,
      kind,
    },
    message: {},
    sessionId,
    type: "message_update",
  });
}

function emitContextMetrics(sessionId, ratio) {
  send({
    compactionCount: 0,
    compactionTokensFreed: 0,
    contextUtilizationRatio: ratio,
    inputTokensUsed: 384,
    preheatInProgress: false,
    preheatResultPending: false,
    sessionId,
    totalToolResultBytesPersisted: 0,
    type: "context_metrics_update",
  });
}

function buildAskQuestionSummary(request, result) {
  if (result.cancelled) {
    return "Manual ask_question skipped by the operator.";
  }

  const parts = result.answers.map((answer) => {
    if (answer.optionIds.includes("__custom__")) {
      return `${answer.questionId}=${String(answer.customText || "").trim()}`;
    }
    const question = request.questions.find((entry) => entry.id === answer.questionId);
    const option = question?.options.find((entry) => entry.id === answer.optionIds[0]);
    return `${answer.questionId}=${option?.label || answer.optionIds[0]}`;
  });

  return `Manual ask_question answers received: ${parts.join("; ")}.`;
}

function emitAskQuestion(sessionId) {
  const requestId = `manual-ask-${askQuestionCounter++}`;
  const request = {
    questions: ASK_QUESTION_PROMPTS,
    requestId,
    responseEvent: `manual.ask_question.response.${requestId}`,
  };
  pendingApprovals.set(requestId, { request, sessionId });
  send({
    payload: request,
    requestId,
    sessionId,
    subtype: "ask_question",
    type: "control_request",
  });
}

function handleApprovalResponse(frame, cancelled = false) {
  const pending = pendingApprovals.get(frame.requestId);
  if (!pending) {
    return;
  }
  pendingApprovals.delete(frame.requestId);

  const session = touchSession(ensureSession(pending.sessionId));
  const payload = frame.payload && typeof frame.payload === "object" ? frame.payload : {};
  const rawResult =
    payload && payload.result && typeof payload.result === "object"
      ? payload.result
      : payload;
  const result = cancelled
    ? { answers: [], cancelled: true }
    : {
        answers: Array.isArray(rawResult.answers) ? rawResult.answers : [],
        cancelled: !!rawResult.cancelled,
      };
  const assistantText = buildAskQuestionSummary(pending.request, result);

  schedule(120, () => emitMessageDelta(pending.sessionId, "content_delta", assistantText));
  schedule(260, () => {
    recordHistoryEntry(session, {
      content: assistantText,
      role: "assistant",
    });
    emitContextMetrics(pending.sessionId, 0.51);
    finishTurn(pending.sessionId, null);
  });
}

function clearTimers() {
  for (const timer of timers) {
    clearTimeout(timer);
  }
  timers.clear();
}

function schedule(delayMs, callback) {
  const timer = setTimeout(() => {
    timers.delete(timer);
    callback();
  }, delayMs);
  timers.add(timer);
}

function finishTurn(sessionId, error = null) {
  const session = touchSession(ensureSession(sessionId));
  clearPendingAssistantMessageId(session);
  session.busy = false;
  persistState();
  send({
    error,
    messages: [],
    sessionId,
    type: "agent_end",
  });
}

function startTurn(sessionId) {
  const session = touchSession(ensureSession(sessionId));
  clearPendingAssistantMessageId(session);
  session.busy = true;
  activeSessionId = sessionId;
  persistState();
  send({
    sessionId,
    type: "agent_start",
  });
}

function buildLiveAssistantPayload(promptText) {
  const thinkingText = [
    "Live thinking: compare the hydrated transcript with the incoming stream so thinking always stays above the assistant reply.",
    "Live thinking: keep following the newest content until the operator scrolls upward, then expose a jump-to-latest affordance.",
    "Live thinking: leave completed tools collapsed unless they fail, because error output deserves immediate visibility.",
  ].join("\n\n");

  const answerText = [
    `Manual acceptance reply for prompt: ${promptText || "empty prompt"}.`,
    "This answer is emitted in multiple deltas so the real VSCode webview can demonstrate automatic bottom-following while content grows.",
    "A complete tool result and a synthetic error tool result will follow, giving you both collapsed and expanded card states to inspect.",
  ].join("\n\n");

  return {
    answerText,
    thinkingText,
    toolCalls: [
      {
        function: { name: "search_workspace" },
        id: "live-tool-search-1",
      },
      {
        function: { name: "validate_layout" },
        id: "live-tool-error-1",
      },
    ],
  };
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

  const promptText = String(frame.text || "");
  const liveAssistant = buildLiveAssistantPayload(promptText);

  send({
    id: frame.id,
    payload: { accepted: true },
    sessionId,
    success: true,
    type: "response",
  });
  startTurn(sessionId);

  recordHistoryEntry(session, {
    content: promptText,
    role: "user",
  });

  if (ASK_QUESTION_REVERIFY_PATTERN.test(promptText)) {
    schedule(160, () => emitAskQuestion(sessionId));
    return;
  }

  if (session.model === "deepseek-v4-pro") {
    schedule(240, () => {
      emitContextMetrics(sessionId, 0.33);
      finishTurn(
        sessionId,
        "LLM调用错误: provider/model 不支持 vision，建议改用 `gpt-5.4`。",
      );
    });
    return;
  }

  const thinkingParts = [
    "Live thinking: compare the hydrated transcript with the incoming stream so thinking stays visibly ahead of the assistant reply.\n\n",
    "Live thinking: auto-scroll should remain attached until the operator scrolls upward, at which point the jump affordance should appear.\n\n",
    "Live thinking: keep error tool output expanded so validation issues are impossible to miss.",
  ];

  const answerParts = [
    `Manual acceptance reply for prompt: ${promptText || "empty prompt"}.\n\n`,
    "This streamed answer adds enough content to make scrolling obvious in the real VSCode sidebar.\n\n",
    "Tool output will now transition through running, streaming, complete, and error states for visual inspection.",
  ];

  schedule(120, () => emitMessageDelta(sessionId, "thinking_delta", thinkingParts[0]));
  schedule(480, () => emitMessageDelta(sessionId, "thinking_delta", thinkingParts[1]));
  schedule(920, () => emitMessageDelta(sessionId, "thinking_delta", thinkingParts[2]));
  schedule(1450, () => emitMessageDelta(sessionId, "content_delta", answerParts[0]));
  schedule(1920, () => emitMessageDelta(sessionId, "content_delta", answerParts[1]));
  schedule(2480, () => emitMessageDelta(sessionId, "content_delta", answerParts[2]));

  schedule(3150, () => {
    send({
      args: { query: "auto scroll tool cards thinking order" },
      sessionId,
      toolCallId: "live-tool-search-1",
      toolName: "search_workspace",
      type: "tool_execution_start",
    });
  });

  schedule(4300, () => {
    send({
      argsPreview: {
        hint: "searching App.tsx, ToolCallCard.tsx, styles.css",
        query: "auto scroll tool cards thinking order",
      },
      sessionId,
      toolCallId: "live-tool-search-1",
      toolName: "search_workspace",
      type: "tool_call_streaming",
    });
  });

  schedule(5600, () => {
    send({
      args: { query: "auto scroll tool cards thinking order" },
      partialResult: {
        matches: [
          "gui/src/App.tsx",
          "gui/src/components/ThinkingBlock.tsx",
          "gui/src/components/ToolCallCard.tsx",
          "gui/src/styles.css",
        ],
        note: "partial matches discovered",
      },
      sessionId,
      toolCallId: "live-tool-search-1",
      toolName: "search_workspace",
      type: "tool_execution_update",
    });
  });

  schedule(7600, () => {
    send({
      display: {
        kind: "text",
        text: "Workspace search completed with a long multiline summary.",
      },
      isError: false,
      result: {
        matches: TOOL_RESULT_BLOCK.split("\n"),
        note: "Expanded tool output should show its own scrollbar once the card body hits the CSS max height.",
        query: "auto scroll tool cards thinking order",
      },
      sessionId,
      toolCallId: "live-tool-search-1",
      toolName: "search_workspace",
      type: "tool_execution_end",
    });
  });

  schedule(8050, () => {
    send({
      args: { width: 420, target: "composer" },
      sessionId,
      toolCallId: "live-tool-error-1",
      toolName: "validate_layout",
      type: "tool_execution_start",
    });
  });

  schedule(9800, () => {
    send({
      display: {
        kind: "text",
        text: "Synthetic validation stderr for the acceptance run.",
      },
      isError: true,
      result: ERROR_TEXT,
      sessionId,
      toolCallId: "live-tool-error-1",
      toolName: "validate_layout",
      type: "tool_execution_end",
    });

    recordHistoryEntry(session, {
      content: liveAssistant.answerText,
      role: "assistant",
      thinking_text: liveAssistant.thinkingText,
      tool_calls: liveAssistant.toolCalls,
    });
    recordHistoryEntry(session, {
      content: TOOL_RESULT_BLOCK,
      role: "tool",
      tool_call_id: "live-tool-search-1",
    });

    emitContextMetrics(sessionId, 0.71);
    finishTurn(sessionId, null);
  });
}

function handleCommand(frame) {
  switch (frame.type) {
    case "control_request":
      if (frame.subtype === "initialize") {
        const sessionId = activeSessionId || createSession();
        send({
          payload: {
            capabilities: [
              "ask_question",
              "prompt",
              "follow_up",
              "new_session",
              "switch_session",
              "list_sessions",
              "get_state",
              "close_session",
              "interrupt",
              "list_models",
              "set_model",
              "set_thinking_level",
            ],
            protocolVersion: 1,
            sessionId,
          },
          requestId: frame.requestId,
          sessionId,
          type: "control_response",
        });
      }
      return;
    case "control_response":
      handleApprovalResponse(frame, false);
      return;
    case "control_cancel":
      handleApprovalResponse(frame, true);
      return;
    case "new_session": {
      const sessionId = createSession();
      send({
        id: frame.id,
        payload: { sessionId },
        sessionId,
        success: true,
        type: "response",
      });
      return;
    }
    case "switch_session": {
      activeSessionId = frame.sessionId || activeSessionId;
      if (activeSessionId) {
        touchSession(ensureSession(activeSessionId));
      }
      persistState();
      send({
        id: frame.id,
        payload: { activeSessionId },
        sessionId: activeSessionId,
        success: true,
        type: "response",
      });
      return;
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
            title: session.title ?? null,
            updatedAt: session.updatedAt,
          })),
        },
        success: true,
        type: "response",
      });
      return;
    case "get_state": {
      const sessionId = frame.sessionId || activeSessionId || createSession();
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
          sessionId,
          sessionKey: session.sessionKey,
          thinkingLevel: currentThinkingLevel(session),
        },
        sessionId,
        success: true,
        type: "response",
      });
      return;
    }
    case "get_messages": {
      const sessionId = frame.sessionId || activeSessionId || createSession();
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
      return;
    }
    case "close_session":
      if (frame.sessionId) {
        sessions.delete(frame.sessionId);
        if (activeSessionId === frame.sessionId) {
          activeSessionId = [...sessions.keys()][0] ?? null;
        }
      }
      persistState();
      send({
        id: frame.id,
        payload: { closed: true, sessionId: frame.sessionId },
        success: true,
        type: "response",
      });
      return;
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
      return;
    case "set_model": {
      const sessionId = frame.sessionId || activeSessionId || createSession();
      const session = touchSession(ensureSession(sessionId));
      session.model = frame.model;
      persistState();
      send({
        id: frame.id,
        payload: {
          model: session.model,
          sessionId,
          thinkingLevel: currentThinkingLevel(session),
        },
        sessionId,
        success: true,
        type: "response",
      });
      return;
    }
    case "set_thinking_level": {
      const sessionId = frame.sessionId || activeSessionId || createSession();
      const session = touchSession(ensureSession(sessionId));
      session.modelThinking = normalizeThinkingLevels(session.modelThinking);
      session.modelThinking[frame.model] = frame.level;
      persistState();
      send({
        id: frame.id,
        payload: {
          level: frame.level,
          model: frame.model,
          sessionId,
        },
        sessionId,
        success: true,
        type: "response",
      });
      return;
    }
    case "prompt":
    case "follow_up":
      handlePrompt(frame);
      return;
    case "interrupt": {
      const sessionId = frame.sessionId || activeSessionId;
      if (sessionId) {
        clearTimers();
        const session = ensureSession(sessionId);
        session.busy = false;
        send({
          partialTextLen: 0,
          sessionId,
          toolResultsCount: 0,
          type: "agent_interrupted",
        });
      }
      send({
        id: frame.id,
        payload: { interrupted: true },
        sessionId,
        success: true,
        type: "response",
      });
      return;
    }
    default:
      send({
        error: `unknown_command: ${frame.type}`,
        id: frame.id || null,
        success: false,
        type: "response",
      });
  }
}

loadState();
if (sessions.size === 0) {
  createSession();
}

const rl = readline.createInterface({
  crlfDelay: Infinity,
  input: process.stdin,
});

rl.on("line", (line) => {
  if (!line.trim()) {
    return;
  }

  let frame;
  try {
    frame = JSON.parse(line);
  } catch (error) {
    send({
      error: `parse_error: ${String((error && error.message) || error)}`,
      success: false,
      type: "response",
    });
    return;
  }

  handleCommand(frame);
});

rl.on("close", () => {
  clearTimers();
});

process.on("SIGINT", () => {
  clearTimers();
  process.exit(0);
});

process.on("SIGTERM", () => {
  clearTimers();
  process.exit(0);
});
