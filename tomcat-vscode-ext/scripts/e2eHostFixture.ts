import * as os from "node:os";
import { readFileSync } from "node:fs";
import * as path from "node:path";
import { chmod, mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";

export interface HostE2eFixture {
  attachmentRootDir: string;
  cleanup(): Promise<void>;
  editFilePath: string;
  env: NodeJS.ProcessEnv;
  fakeServePath: string;
  rootDir: string;
  workspaceDir: string;
}

export interface HostE2eFixtureOptions {
  bootstrapListModelsFailures?: number;
  handshakeDelayMs?: number;
  interruptDelayMs?: number;
  requireInit?: boolean;
  transientServeFailures?: number;
}

const FALLBACK_SERVER_VERSION = "0.1.30";

function readBundledCliVersion(): string {
  try {
    const metadata = JSON.parse(
      readFileSync(new URL("../package.json", import.meta.url), "utf8"),
    ) as { tomcat?: { bundledCliVersion?: unknown } };
    const version = metadata.tomcat?.bundledCliVersion;
    return typeof version === "string" && version.trim().length > 0
      ? version.trim()
      : FALLBACK_SERVER_VERSION;
  } catch {
    return FALLBACK_SERVER_VERSION;
  }
}

const BUNDLED_CLI_VERSION = readBundledCliVersion();

const CHAT_E2E_SETTINGS = {
  "chat.allowAnonymousAccess": true,
  "chat.disableAIFeatures": false,
  "chat.mcp.discovery.enabled": false,
  "chat.mcp.enabled": false,
  // Keep side-by-side enabled, but leave the inline breakpoint untouched so the
  // runtime diff fix is what forces narrow editors to stay double-pane.
  "diffEditor.renderSideBySide": true,
  "github.copilot.chat.githubMcpServer.enabled": false,
  "workbench.startupEditor": "none",
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

function buildFakeServeSource(
  editFilePath: string,
  options: HostE2eFixtureOptions,
  setupMarkerPath: string,
): string {
  const transcriptPlanMarkdown = JSON.stringify(`---
name: Transcript UI Showcase
overview: Review the transcript UI polish and confirm the merged plan card before building.
---

# Transcript UI showcase
`);
  const planToolUxMarkdown = JSON.stringify(`---
name: plan tool ux
overview: Keep one Creating plan header and a breathing View Plan state.
---

# Plan tool UX
`);
  return `#!/usr/bin/env node
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");
const readline = require("node:readline");

const editFilePath =
  process.env.TOMCAT_VSCODE_TEST_EDIT_FILE || ${JSON.stringify(editFilePath)};
const requireInit =
  process.env.TOMCAT_VSCODE_TEST_REQUIRE_INIT === "1"
    ? true
    : ${JSON.stringify(Boolean(options.requireInit))};
const transientServeFailures = Math.max(
  0,
  Number(
    process.env.TOMCAT_VSCODE_TEST_TRANSIENT_SERVE_FAILURES
      || ${JSON.stringify(options.transientServeFailures ?? 0)},
  ),
);
const handshakeDelayMs = Math.max(
  0,
  Number(
    process.env.TOMCAT_VSCODE_TEST_HANDSHAKE_DELAY_MS
      || ${JSON.stringify(options.handshakeDelayMs ?? 0)},
  ),
);
let bootstrapListModelsFailures = Math.max(
  0,
  Number(
    process.env.TOMCAT_VSCODE_TEST_BOOTSTRAP_LIST_MODELS_FAILURES
      || ${JSON.stringify(options.bootstrapListModelsFailures ?? 0)},
  ),
);
const interruptDelayMs = Math.max(
  0,
  Number(
    process.env.TOMCAT_VSCODE_TEST_INTERRUPT_DELAY_MS
      ?? ${JSON.stringify(options.interruptDelayMs ?? 150)},
  ) || 0,
);
const setupMarkerPath =
  process.env.TOMCAT_VSCODE_TEST_SETUP_MARKER || ${JSON.stringify(setupMarkerPath)};
const transientFailureMarkerPath =
  process.env.TOMCAT_VSCODE_TEST_TRANSIENT_FAILURE_MARKER
  || ${JSON.stringify(`${setupMarkerPath}.transient-failures`)};
// Attachment bytes live outside the extension, exactly as they do with a real backend,
// so the host has to be told where they are and grant the webview access to that path.
const ATTACHMENT_ROOT =
  process.env.TOMCAT_VSCODE_TEST_ATTACHMENT_ROOT ||
  path.join(path.dirname(setupMarkerPath), "..", "attachments");
const BLOBS_DIR = path.join(ATTACHMENT_ROOT, "blobs");
const THUMBS_DIR = path.join(ATTACHMENT_ROOT, "thumbs");
const BUILTIN_MODELS = [
  {
    api: "openai",
    apiKeyEnv: "OPENAI_API_KEY",
    capabilities: { files: false, reasoning: false, tools: true, vision: false, webSearch: false },
    id: "fake-model",
    keyPresent: true,
    modelName: "fake-model",
    provider: "openai",
    source: "builtin",
    supportedReasoningLevels: [],
    thinkingFormat: null,
  },
  {
    api: "openai",
    apiKeyEnv: "OPENAI_API_KEY",
    capabilities: { files: false, reasoning: true, tools: true, vision: false, webSearch: false },
    id: "gpt-5.4",
    keyPresent: true,
    modelName: "gpt-5.4",
    provider: "openai",
    source: "builtin",
    supportedReasoningLevels: ["low", "medium", "high", "xhigh"],
    thinkingFormat: "openai",
  },
  {
    api: "anthropic-messages",
    apiKeyEnv: "ANTHROPIC_API_KEY",
    capabilities: { files: false, reasoning: true, tools: true, vision: true, webSearch: false },
    id: "claude-4.6-sonnet",
    keyPresent: true,
    modelName: "claude-4.6-sonnet",
    provider: "anthropic",
    source: "builtin",
    supportedReasoningLevels: ["low", "medium", "high", "max"],
    thinkingFormat: "anthropic-adaptive",
  },
];
const MODEL_OPTIONS = BUILTIN_MODELS.map((model) => model.id);
const userModels = new Map();
const modelPreferences = new Map();
const providerKeys = new Map(
  BUILTIN_MODELS.map((model) => [model.apiKeyEnv, { keyPresent: true, provider: model.provider }]),
);
const USER_MODELS_PATH = path.join(path.dirname(setupMarkerPath), "fake-serve-models.json");
const MODEL_PREFS_PATH = path.join(path.dirname(setupMarkerPath), "fake-serve-model-prefs.json");
try {
  const savedModels = JSON.parse(fs.readFileSync(USER_MODELS_PATH, "utf8"));
  if (Array.isArray(savedModels)) {
    for (const model of savedModels) {
      if (model && typeof model.id === "string") {
        userModels.set(model.id, model);
        if (typeof model.selectedReasoningLevel === "string") {
          modelPreferences.set(model.id, { reasoning: model.selectedReasoningLevel });
        }
        if (model.keyPresent === true && typeof model.apiKeyEnv === "string") {
          providerKeys.set(model.apiKeyEnv, { keyPresent: true, provider: model.provider || "openai" });
        }
      }
    }
  }
} catch (error) {
  if (!error || error.code !== "ENOENT") throw error;
}
try {
  const savedPreferences = JSON.parse(fs.readFileSync(MODEL_PREFS_PATH, "utf8"));
  if (savedPreferences && typeof savedPreferences === "object") {
    for (const [modelId, preference] of Object.entries(savedPreferences)) {
      if (preference && typeof preference === "object") {
        modelPreferences.set(modelId, {
          ...modelPreferences.get(modelId),
          ...preference,
        });
      }
    }
  }
} catch (error) {
  if (!error || error.code !== "ENOENT") throw error;
}
function persistUserModels() {
  fs.mkdirSync(path.dirname(USER_MODELS_PATH), { recursive: true });
  fs.writeFileSync(USER_MODELS_PATH, JSON.stringify([...userModels.values()]));
}
function persistModelPreferences() {
  fs.mkdirSync(path.dirname(MODEL_PREFS_PATH), { recursive: true });
  fs.writeFileSync(MODEL_PREFS_PATH, JSON.stringify(Object.fromEntries(modelPreferences)));
}
const sessions = new Map();
const attachmentLeases = new Map();
let sessionCounter = 1;
let historyCounter = 1;
let assistantMessageCounter = 1;
let pendingApproval = null;
let pendingInterrupt = null;
let pendingPlanCompletion = null;
let activeSessionId = null;
const RESUME_STATE_PATH = path.join(path.dirname(setupMarkerPath), "fake-serve-resume.json");
const debugGetMessages = process.env.TOMCAT_E2E_DEBUG_GET_MESSAGES === "1";
const historyPageDelayMs = Math.max(
  0,
  Number(process.env.TOMCAT_E2E_HISTORY_PAGE_DELAY_MS || "120"),
);
const transcriptProgressDelayMs = Math.max(
  0,
  Number(process.env.TOMCAT_E2E_TRANSCRIPT_PROGRESS_DELAY_MS || "1000"),
);
const serverVersion = ${JSON.stringify(BUNDLED_CLI_VERSION)};
function persistPendingApproval() {
  if (!pendingApproval || pendingApproval.kind !== "answer-card") {
    try {
      fs.unlinkSync(RESUME_STATE_PATH);
    } catch (error) {
      if (!error || error.code !== "ENOENT") {
        throw error;
      }
    }
    return;
  }
  fs.mkdirSync(path.dirname(RESUME_STATE_PATH), { recursive: true });
  fs.writeFileSync(RESUME_STATE_PATH, JSON.stringify({
    activeSessionId,
    assistantMessageCounter,
    historyCounter,
    pendingApproval,
    sessionCounter,
    sessions: [...sessions.entries()],
  }));
}

function restorePendingApproval() {
  if (!fs.existsSync(RESUME_STATE_PATH)) {
    return;
  }
  try {
    const state = JSON.parse(fs.readFileSync(RESUME_STATE_PATH, "utf8"));
    if (!state || !Array.isArray(state.sessions) || !state.pendingApproval) {
      return;
    }
    for (const [sessionId, session] of state.sessions) {
      if (typeof sessionId === "string" && session && typeof session === "object") {
        sessions.set(sessionId, session);
      }
    }
    activeSessionId = typeof state.activeSessionId === "string" ? state.activeSessionId : null;
    pendingApproval = state.pendingApproval;
    sessionCounter = Number.isInteger(state.sessionCounter) ? state.sessionCounter : sessionCounter;
    historyCounter = Number.isInteger(state.historyCounter) ? state.historyCounter : historyCounter;
    assistantMessageCounter = Number.isInteger(state.assistantMessageCounter)
      ? state.assistantMessageCounter
      : assistantMessageCounter;
  } catch {
    // A corrupt synthetic state must not make unrelated extension-host tests fail.
  }
}

if (process.argv[2] === "--version") {
  process.stdout.write("tomcat fake " + serverVersion + "\\n");
  process.exit(0);
}

if (process.argv[2] === "init") {
  fs.mkdirSync(path.dirname(setupMarkerPath), { recursive: true });
  fs.writeFileSync(setupMarkerPath, "ok\\n", "utf8");
  process.stdout.write("fake init completed\\n");
  process.exit(0);
}

if (process.argv[2] !== "serve") {
  process.stderr.write("unsupported command: " + String(process.argv[2] || "") + "\\n");
  process.exit(2);
}

if (requireInit && !fs.existsSync(setupMarkerPath)) {
  process.stderr.write("fake serve requires tomcat init first\\n");
  process.exit(1);
}

if (transientServeFailures > 0) {
  let failuresSoFar = 0;
  try {
    failuresSoFar = Number(fs.readFileSync(transientFailureMarkerPath, "utf8")) || 0;
  } catch (error) {
    if (!error || error.code !== "ENOENT") throw error;
  }
  if (failuresSoFar < transientServeFailures) {
    fs.mkdirSync(path.dirname(transientFailureMarkerPath), { recursive: true });
    fs.writeFileSync(transientFailureMarkerPath, String(failuresSoFar + 1));
    process.stderr.write("fake transient serve startup failure\\n");
    process.exit(1);
  }
}

function touchSession(session) {
  session.updatedAt = Date.now();
  return session;
}

function inferDefaultKeyEnv(provider) {
  if (typeof provider !== "string" || provider.trim().length === 0) {
    return "OPENAI_API_KEY";
  }
  return provider.trim().toUpperCase().replace(/[^A-Z0-9]+/g, "_") + "_API_KEY";
}

function normalizeThinkingFormat(api, thinkingFormat) {
  if (typeof thinkingFormat === "string" && thinkingFormat.trim().length > 0) {
    return thinkingFormat.trim().toLowerCase();
  }
  switch (String(api || "").trim()) {
    case "deepseek":
      return "deepseek";
    case "zai":
      return "zai";
    case "qwen":
      return "qwen";
    case "doubao":
    case "moonshot":
      return "doubao";
    case "anthropic":
      return "anthropic";
    case "anthropic-messages":
      return "anthropic";
    case "openai":
    case "openai-responses":
    default:
      return "openai";
  }
}

function normalizeReasoningLevel(value) {
  const token = String(value || "").trim().toLowerCase();
  if (token === "x-high") {
    return "xhigh";
  }
  if (token === "none") {
    return "off";
  }
  return token;
}

function inferSupportedReasoningLevels(api, thinkingFormat) {
  switch (normalizeThinkingFormat(api, thinkingFormat)) {
    case "deepseek":
    case "zai":
      return ["high", "max"];
    case "doubao":
      return [];
    case "anthropic":
    case "anthropic-adaptive":
      return ["low", "medium", "high", "xhigh", "max"];
    case "qwen":
    case "openrouter":
    case "openai":
    default:
      return ["low", "medium", "high", "xhigh"];
  }
}

function normalizeSupportedReasoningLevels(levels, api, thinkingFormat) {
  if (!Array.isArray(levels)) {
    return inferSupportedReasoningLevels(api, thinkingFormat);
  }
  const normalized = [];
  for (const value of levels) {
    const token = normalizeReasoningLevel(value);
    if (!token || normalized.includes(token)) {
      continue;
    }
    normalized.push(token);
  }
  return normalized.length > 0 ? normalized : inferSupportedReasoningLevels(api, thinkingFormat);
}

function collectModelWarnings(api, thinkingFormat) {
  const format = normalizeThinkingFormat(api, thinkingFormat);
  const effortApis = new Set(["openai", "openai-responses"]);
  const effortFormats = new Set(["openai", "openrouter", "deepseek", "zai"]);
  if (!effortApis.has(String(api || "").trim()) || effortFormats.has(format)) {
    return [];
  }
  return [
    'Current API "' +
      String(api || "").trim() +
      '" will not send reasoning effort when thinking format is "' +
      format +
      '".',
  ];
}

function normalizeModelEntry(entry, source) {
  const provider = typeof entry?.provider === "string" ? entry.provider : "openai";
  const api = typeof entry?.api === "string" ? entry.api : "openai";
  const apiKeyEnv =
    typeof entry?.apiKeyEnv === "string" && entry.apiKeyEnv.trim().length > 0
      ? entry.apiKeyEnv.trim()
      : inferDefaultKeyEnv(provider);
  const providerKey = providerKeys.get(apiKeyEnv);
  const thinkingFormat = normalizeThinkingFormat(api, entry?.thinkingFormat);
  const supportedReasoningLevels = normalizeSupportedReasoningLevels(
    entry?.supportedReasoningLevels,
    api,
    thinkingFormat,
  );
  const id = typeof entry?.id === "string" ? entry.id : "fake-model";
  const preference = modelPreferences.get(id) || {};
  const contextWindow = typeof entry?.contextWindow === "number" ? entry.contextWindow : null;
  const contextWindowOptions = Array.isArray(entry?.contextWindowOptions)
    ? entry.contextWindowOptions.filter((value) => Number.isInteger(value) && value > 0)
    : [];
  return {
    api,
    apiKeyEnv,
    baseUrl: typeof entry?.baseUrl === "string" ? entry.baseUrl : null,
    capabilities: {
      files: entry?.capabilities?.files === true,
      reasoning: entry?.capabilities?.reasoning === true,
      tools: entry?.capabilities?.tools === true,
      vision: entry?.capabilities?.vision === true,
      webSearch:
        entry?.capabilities?.webSearch === true || entry?.capabilities?.web_search === true,
    },
    contextWindow,
    contextWindowOptions,
    description: typeof entry?.description === "string" ? entry.description : null,
    cost:
      entry?.cost && typeof entry.cost === "object"
        ? {
            inputPerMtok:
              typeof entry.cost.inputPerMtok === "number"
                ? entry.cost.inputPerMtok
                : typeof entry.cost.input_per_mtok === "number"
                  ? entry.cost.input_per_mtok
                  : null,
            outputPerMtok:
              typeof entry.cost.outputPerMtok === "number"
                ? entry.cost.outputPerMtok
                : typeof entry.cost.output_per_mtok === "number"
                  ? entry.cost.output_per_mtok
                  : null,
          }
        : null,
    id,
    selectedContextWindow:
      typeof preference.contextWindow === "number" ? preference.contextWindow : contextWindow,
    selectedReasoningLevel:
      typeof preference.reasoning === "string" ? preference.reasoning : null,
    keyPresent: providerKey?.keyPresent === true,
    modelName: typeof entry?.modelName === "string" ? entry.modelName : null,
    provider,
    source,
    supportedReasoningLevels,
    thinkingFormat,
  };
}

function listModelViews() {
  return [
    ...BUILTIN_MODELS.map((model) => normalizeModelEntry(model, "builtin")),
    ...Array.from(userModels.values()).map((model) => normalizeModelEntry(model, "user")),
  ];
}

function listProviderKeyViews() {
  const grouped = new Map();
  for (const model of listModelViews()) {
    if (!grouped.has(model.apiKeyEnv)) {
      grouped.set(model.apiKeyEnv, {
        envName: model.apiKeyEnv,
        keyPresent: model.keyPresent,
        modelIds: [],
        provider: model.provider,
      });
    }
    grouped.get(model.apiKeyEnv).keyPresent = model.keyPresent;
    grouped.get(model.apiKeyEnv).modelIds.push(model.id);
  }
  return Array.from(grouped.values());
}

function createSession() {
  const sessionId = \`session-\${sessionCounter++}\`;
  sessions.set(sessionId, touchSession({
    busy: false,
    agentMode: "chat",
    contextRatio: null,
    cwd: process.cwd(),
    history: [],
    mode: "code",
    model: MODEL_OPTIONS[0],
    pendingAssistantMessageId: null,
    planId: null,
    planPath: null,
    planState: "chat",
    sessionKey: "fake-workspace",
    thinkingByModel: {},
    title: null,
  }));
  if (!activeSessionId) {
    activeSessionId = sessionId;
  }
  return sessionId;
}

restorePendingApproval();
if (sessions.size === 0) {
  createSession();
}

function send(frame) {
  process.stdout.write(JSON.stringify(frame) + "\\n");
}

function activePlanForSession(session) {
  if (!session.planId || !session.planPath || session.planState === "chat") {
    return null;
  }
  return {
    id: session.planId,
    path: session.planPath,
    state: session.planState,
  };
}

function planControlPayload(session) {
  return {
    activePlan: activePlanForSession(session),
    agentMode: session.agentMode || "chat",
  };
}

function emitAgentModeChanged(sessionId) {
  const session = ensureSession(sessionId);
  send({
    agentMode: session.agentMode || "chat",
    sessionId,
    type: "session.agent_mode.changed",
  });
}

function deriveSessionTitleFromText(text) {
  if (typeof text !== "string") {
    return null;
  }
  const firstLine = text
    .split(/\\r?\\n/u)
    .map((line) => line.trim())
    .find((line) => line.length > 0);
  if (!firstLine) {
    return null;
  }
  if ([...firstLine].length > 40) {
    return [...firstLine].slice(0, 40).join("") + "…";
  }
  return firstLine;
}

function extractUserTextForTitle(content) {
  if (typeof content === "string") {
    return content;
  }
  if (!Array.isArray(content)) {
    return null;
  }
  let text = "";
  let sawInputText = false;
  for (const part of content) {
    if (!part || typeof part !== "object" || part.type !== "input_text") {
      continue;
    }
    if (typeof part.text === "string") {
      text += part.text;
      sawInputText = true;
    }
  }
  return sawInputText ? text : null;
}

function emitSessionTitleUpdated(sessionId, content) {
  const title = deriveSessionTitleFromText(extractUserTextForTitle(content));
  if (!title) {
    return;
  }
  const session = touchSession(ensureSession(sessionId));
  if (session.title && session.title !== "New session") {
    return;
  }
  session.title = title;
  send({
    sessionId,
    title,
    type: "session.title_updated",
  });
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
      pendingAssistantMessageId: null,
      planId: null,
      planPath: null,
      planState: "chat",
      sessionKey: "fake-workspace",
      thinkingByModel: {},
      title: null,
    }));
  }
  return sessions.get(sessionId);
}

function nextAssistantMessageId() {
  return \`assistant-\${assistantMessageCounter++}\`;
}

function ensurePendingAssistantMessageId(session) {
  if (!session.pendingAssistantMessageId) {
    session.pendingAssistantMessageId = nextAssistantMessageId();
  }
  return session.pendingAssistantMessageId;
}

function clearPendingAssistantMessageId(sessionId) {
  ensureSession(sessionId).pendingAssistantMessageId = null;
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

function createAddedDiff(lines) {
  return lines.map((text, index) => ({
    tag: "add",
    oldLine: null,
    newLine: index + 1,
    text,
  }));
}

function createReplacementDiff(oldLines, newLines) {
  const diff = [];
  let oldLine = 1;
  let newLine = 1;
  const total = Math.max(oldLines.length, newLines.length);
  for (let index = 0; index < total; index += 1) {
    const before = oldLines[index];
    const after = newLines[index];
    if (before !== undefined && after !== undefined && before === after) {
      diff.push({
        tag: "ctx",
        oldLine,
        newLine,
        text: before,
      });
      oldLine += 1;
      newLine += 1;
      continue;
    }
    if (before !== undefined) {
      diff.push({
        tag: "del",
        oldLine,
        newLine: null,
        text: before,
      });
      oldLine += 1;
    }
    if (after !== undefined) {
      diff.push({
        tag: "add",
        oldLine: null,
        newLine,
        text: after,
      });
      newLine += 1;
    }
  }
  return diff;
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
      display: {
        added: 2,
        diff: createAddedDiff([
          "export const created = true;",
          "console.log('done');",
        ]),
        file: outputPath,
        kind: "file",
        removed: 0,
      },
      result: "Created output.txt",
    },
    {
      toolCallId: "tc-showcase-edit",
      toolName: "edit",
      args: { path: sourcePath },
      display: {
        added: 2,
        diff: createReplacementDiff(
          [
            "export const title = 'before';",
            "export const count = 1;",
          ],
          [
            "export const title = 'after';",
            "export const count = 2;",
          ],
        ),
        file: sourcePath,
        kind: "file",
        removed: 2,
      },
      result: "Edited src/app.tsx",
    },
    {
      toolCallId: "tc-showcase-hashline-edit",
      toolName: "hashline_edit",
      args: { path: sourcePath },
      display: {
        added: 1,
        diff: createReplacementDiff(
          [
            "function keep() {",
            "  return 'old';",
            "}",
          ],
          [
            "function keep() {",
            "  return 'new';",
            "}",
          ],
        ),
        file: sourcePath,
        kind: "file",
        removed: 1,
      },
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
    switch (index % 3) {
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

function buildEditDisplayReplayTools() {
  const fixtureDir = path.join(process.cwd(), "test-stuff", "edit-display-replay");
  fs.mkdirSync(fixtureDir, { recursive: true });
  const singlePath = path.join(fixtureDir, "single.ts");
  const batchSinglePath = path.join(fixtureDir, "batch-single.ts");
  const multiAppliedPath = path.join(fixtureDir, "multi-applied.ts");
  const multiFailedPath = path.join(fixtureDir, "multi-failed.ts");
  fs.writeFileSync(singlePath, "export const mode = 'before';\\n", "utf8");
  fs.writeFileSync(batchSinglePath, "export const batch = 'before';\\n", "utf8");
  fs.writeFileSync(multiAppliedPath, "export const multi = 'before';\\n", "utf8");
  fs.writeFileSync(multiFailedPath, "export const untouched = true;\\n", "utf8");
  return [
    {
      toolCallId: "tc-edit-display-single",
      toolName: "edit",
      args: { path: singlePath },
      display: {
        added: 1,
        diff: createReplacementDiff(
          ["export const mode = 'before';"],
          ["export const mode = 'after';"],
        ),
        file: singlePath,
        kind: "file",
        removed: 1,
      },
      result: "已编辑: " + singlePath,
    },
    {
      toolCallId: "tc-edit-display-batch-single",
      toolName: "edit",
      args: {
        files: [
          {
            new_content: "export const batch = 'after';",
            old_content: "export const batch = 'before';",
            path: batchSinglePath,
          },
        ],
      },
      display: {
        added: 1,
        diff: createReplacementDiff(
          ["export const batch = 'before';"],
          ["export const batch = 'after';"],
        ),
        file: batchSinglePath,
        kind: "file",
        removed: 1,
      },
      result: "已编辑 1 个文件，全部落盘\\n- APPLIED " + batchSinglePath + " (+1 -1)",
    },
    {
      toolCallId: "tc-edit-display-batch-multi",
      toolName: "edit",
      args: {
        files: [
          {
            new_content: "export const multi = 'after';",
            old_content: "export const multi = 'before';",
            path: multiAppliedPath,
          },
          {
            new_content: "export const untouched = false;",
            old_content: "export const untouched = true;",
            path: multiFailedPath,
          },
        ],
      },
      display: {
        files: [
          {
            added: 1,
            diff: createReplacementDiff(
              ["export const multi = 'before';"],
              ["export const multi = 'after';"],
            ),
            file: multiAppliedPath,
            removed: 1,
            status: "applied",
          },
          {
            file: multiFailedPath,
            note: "第 2 段匹配到 3 处",
            status: "failed",
          },
        ],
        kind: "files",
        summary: "1 个文件已落盘，1 个失败且未写入；失败的文件磁盘内容保持原样",
      },
      result:
        "1 个文件已落盘，1 个失败且未写入；失败的文件磁盘内容保持原样\\n" +
        "- APPLIED " + multiAppliedPath + " (+1 -1)\\n" +
        "- FAILED  " + multiFailedPath + ": 第 2 段匹配到 3 处",
    },
  ];
}

function emitMessageDelta(sessionId, delta) {
  const session = touchSession(ensureSession(sessionId));
  const assistantMessageId = ensurePendingAssistantMessageId(session);
  send({
    assistantMessageId,
    assistantMessageEvent: {
      delta,
      kind: "content_delta",
    },
    message: {},
    sessionId,
    type: "message_update",
  });
}

function emitTurnEnd(sessionId, frame = {}) {
  const session = touchSession(ensureSession(sessionId));
  const assistantMessageId = ensurePendingAssistantMessageId(session);
  send({
    assistantMessageId,
    sessionId,
    ...frame,
    type: "turn_end",
  });
  return assistantMessageId;
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

function normalizeHistoryContent(text, segments) {
  if (!Array.isArray(segments) || segments.length === 0) {
    return text;
  }
  const normalized = segments
    .map((segment) => {
      if (!segment || typeof segment !== "object") {
        return null;
      }
      if (segment.type === "text" && typeof segment.text === "string") {
        return {
          text: segment.text,
          type: "input_text",
        };
      }
      if (
        segment.type === "reference"
        && (segment.kind === "selection" || segment.kind === "file")
        && typeof segment.path === "string"
        && typeof segment.label === "string"
      ) {
        return {
          label: segment.label,
          line_end: typeof segment.lineEnd === "number" ? segment.lineEnd : undefined,
          line_start: typeof segment.lineStart === "number" ? segment.lineStart : undefined,
          path: segment.path,
          ref_kind: segment.kind,
          text: typeof segment.text === "string" ? segment.text : undefined,
          type: "input_reference",
        };
      }
      return null;
    })
    .filter(Boolean);
  return normalized.length > 0 ? normalized : text;
}

function recordHistoryMessage(sessionId, role, content, forcedId = null) {
  const session = touchSession(ensureSession(sessionId));
  session.history.push({
    id:
      forcedId ||
      (role === "assistant"
        ? ensurePendingAssistantMessageId(session)
        : "h-" + historyCounter++),
    message: {
      content,
      role,
    },
    type: "message",
  });
}

function markLatestUserMessageFailed(sessionId) {
  const session = touchSession(ensureSession(sessionId));
  for (let index = session.history.length - 1; index >= 0; index -= 1) {
    const entry = session.history[index];
    if (entry?.type !== "message" || entry?.message?.role !== "user") {
      continue;
    }
    entry.message.superseded = true;
    entry.message.turn_failed = true;
    return entry;
  }
  return null;
}

function recordHistoryCustom(sessionId, event, attempt) {
  const session = touchSession(ensureSession(sessionId));
  session.history.push({
    attempt,
    event,
    type: "custom",
  });
}

function copyUserMessageForward(sessionId, messageId) {
  const session = touchSession(ensureSession(sessionId));
  const targetIndex = session.history.findIndex(
    (entry) => entry?.type === "message" && entry?.id === messageId && entry?.message?.role === "user",
  );
  if (targetIndex < 0) {
    return null;
  }
  for (let index = targetIndex + 1; index < session.history.length; index += 1) {
    const entry = session.history[index];
    if (
      entry?.type === "message"
      && entry?.message?.role === "user"
      && entry?.message?.superseded !== true
    ) {
      return null;
    }
  }
  const message = JSON.parse(JSON.stringify(session.history[targetIndex].message));
  delete message.superseded;
  delete message.turn_failed;
  const copy = {
    id: "h-" + historyCounter++,
    message,
    type: "message",
  };
  session.history.push(copy);
  return copy;
}

function recordHistoryError(sessionId, summary, detail) {
  const session = touchSession(ensureSession(sessionId));
  session.history.push({
    detail,
    id: "error-" + String(historyCounter++),
    summary,
    type: "error",
  });
}

function seedTranscriptSwitchBackHistory(sessionId) {
  const session = touchSession(ensureSession(sessionId));
  if (session.seededSwitchBackHistory) {
    return;
  }
  session.seededSwitchBackHistory = true;
  for (let index = 0; index < 5; index += 1) {
    recordHistoryMessage(sessionId, "user", "ghost prompt " + String(index + 1));
  }
  for (let index = 0; index < 81; index += 1) {
    recordHistoryMessage(sessionId, "user", "history filler " + String(index + 1));
  }
}

function recordHistoryAssistantWithTools(sessionId, content, tools, summaryTitle) {
  const session = touchSession(ensureSession(sessionId));
  session.history.push({
    id: ensurePendingAssistantMessageId(session),
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
      tool_display: tool.display,
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
    providerUsageMeasured: true,
    sessionId,
    totalToolResultBytesPersisted: 0,
    type: "context_metrics_update",
  });
}

function emitEstimatedContextMetrics(sessionId, ratio) {
  const session = touchSession(ensureSession(sessionId));
  if (typeof session.contextRatio === "number") {
    session.contextRatio = ratio;
  }
  send({
    compactionCount: 0,
    compactionTokensFreed: 0,
    contextUtilizationRatio: ratio,
    inputTokensUsed: 256,
    preheatInProgress: false,
    preheatResultPending: false,
    providerUsageMeasured: false,
    sessionId,
    totalToolResultBytesPersisted: 0,
    type: "context_metrics_update",
  });
}

function finishTurn(sessionId, error = null) {
  const session = touchSession(ensureSession(sessionId));
  clearPendingAssistantMessageId(sessionId);
  session.busy = false;
  send({
    error,
    messages: [],
    sessionId,
    type: "agent_end",
  });
  send({
    sessionId,
    type: "agent_idle",
  });
}

function startTurn(sessionId) {
  const session = touchSession(ensureSession(sessionId));
  clearPendingAssistantMessageId(sessionId);
  session.busy = true;
  activeSessionId = sessionId;
  send({
    sessionId,
    type: "agent_start",
  });
}

/** Write bytes under their own sha256, the way the real blob store does. */
function putBlob(bytes) {
  const sha = crypto.createHash("sha256").update(bytes).digest("hex");
  const target = path.join(BLOBS_DIR, sha);
  if (!fs.existsSync(target)) {
    fs.mkdirSync(BLOBS_DIR, { recursive: true });
    fs.writeFileSync(target, bytes);
  }
  return sha;
}

/** Thumbnails are addressed by the hash of the image they came from, never their own. */
function putThumbnail(sourceSha, bytes) {
  fs.mkdirSync(THUMBS_DIR, { recursive: true });
  fs.writeFileSync(path.join(THUMBS_DIR, sourceSha), bytes);
}

function handleIngestAttachment(frame) {
  const sessionId = frame.sessionId || activeSessionId || createSession();
  const attachment = (frame && frame.attachment) || {};
  if (
    typeof attachment.dataBase64 !== "string" ||
    typeof attachment.mimeType !== "string"
  ) {
    send({
      error: "ingest_attachment requires dataBase64 and mimeType",
      id: frame.id,
      sessionId,
      success: false,
      type: "response",
    });
    return;
  }

  const bytes = Buffer.from(attachment.dataBase64, "base64");
  const blobSha = putBlob(bytes);
  let hasThumb = false;
  if (typeof attachment.thumbBase64 === "string" && attachment.thumbBase64.length > 0) {
    putThumbnail(blobSha, Buffer.from(attachment.thumbBase64, "base64"));
    hasThumb = true;
  }
  const providerSha =
    typeof attachment.providerBase64 === "string" && attachment.providerBase64.length > 0
      ? putBlob(Buffer.from(attachment.providerBase64, "base64"))
      : null;

  send({
    id: frame.id,
    payload: {
      blobSha,
      bytes: bytes.length,
      filename:
        typeof attachment.filename === "string" && attachment.filename.length > 0
          ? attachment.filename
          : "pasted-image.png",
      hasThumb,
      mimeType: attachment.mimeType,
      providerSha,
    },
    sessionId,
    success: true,
    type: "response",
  });
}

function handleCacheAttachmentThumbnail(frame) {
  const sessionId = frame.sessionId || activeSessionId || createSession();
  const thumbnail = (frame && frame.thumbnail) || {};
  const cached =
    typeof thumbnail.sourceSha === "string" &&
    /^[0-9a-f]{64}$/.test(thumbnail.sourceSha) &&
    typeof thumbnail.thumbBase64 === "string" &&
    thumbnail.thumbBase64.length > 0;
  if (cached) {
    putThumbnail(thumbnail.sourceSha, Buffer.from(thumbnail.thumbBase64, "base64"));
  }
  send({
    id: frame.id,
    payload: { cached },
    sessionId,
    success: true,
    type: "response",
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
  const segments = Array.isArray(frame.params && frame.params.segments)
    ? frame.params.segments
    : null;
  const userMessageId =
    frame.params && typeof frame.params.userMessageId === "string"
      ? frame.params.userMessageId
      : null;
  if (text.includes("switch back order")) {
    seedTranscriptSwitchBackHistory(sessionId);
  }
  const normalizedUserContent = normalizeHistoryContent(text, segments);
  if (!frame.resume) {
    recordHistoryMessage(sessionId, "user", normalizedUserContent, userMessageId);
    emitSessionTitleUpdated(sessionId, normalizedUserContent);
  }
  if (text.includes("waterline pending second request")) {
    emitEstimatedContextMetrics(sessionId, 0.53);
    setTimeout(() => {
      emitMessageDelta(sessionId, "waterline refreshed");
      recordHistoryMessage(sessionId, "assistant", "waterline refreshed");
      emitContextMetrics(sessionId, 0.57);
      finishTurn(sessionId, null);
    }, 300);
    return;
  }
  if (text.includes("answer card showcase")) {
    // Every tool invocation owns a distinct durable id, even in the same session.
    const requestId = \`ask-answer-\${sessionId}-\${crypto.randomUUID()}\`;
    const toolCallId = \`tool-ask-\${requestId}\`;
    const request = {
      questions: [
        {
          id: "deploy-target",
          options: [
            { id: "staging", label: "Staging", recommended: true },
            { id: "production", label: "Production", recommended: false },
          ],
          prompt: "Deploy where?",
        },
      ],
      requestId,
      responseEvent: \`plan.ask_question.response.\${requestId}\`,
      toolCallId,
    };
    const pendingTool = {
      args: { questions: request.questions },
      result: "[pending]",
      toolCallId,
      toolName: "ask_question",
    };
    recordHistoryAssistantWithTools(sessionId, "", [pendingTool], "Asked question");
    recordHistoryToolResult(sessionId, pendingTool);
    pendingApproval = { kind: "answer-card", request, requestId, sessionId };
    persistPendingApproval();
    send({
      args: { questions: request.questions },
      sessionId,
      toolCallId,
      toolName: "ask_question",
      type: "tool_execution_start",
    });
    send({
      payload: request,
      requestId,
      sessionId,
      subtype: "ask_question",
      type: "control_request",
    });
    return;
  }
  if (text.includes("approve edit")) {
    const requestId = \`ask-\${sessionId}\`;
    const request = {
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
    };
    pendingApproval = { kind: "edit-approval", request, requestId, sessionId };
    send({
      payload: request,
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

  if (text.includes("loading gap showcase")) {
    const assistantMessageId = ensurePendingAssistantMessageId(session);
    setTimeout(() => {
      send({
        assistantMessageEvent: {
          delta: "Working through the first response.",
          kind: "thinking_delta",
        },
        assistantMessageId,
        message: {},
        sessionId,
        type: "message_update",
      });
    }, 250);
    setTimeout(() => {
      emitMessageDelta(sessionId, "loading gap complete");
      recordHistoryMessage(sessionId, "assistant", "loading gap complete");
      emitContextMetrics(sessionId, 0.33);
      finishTurn(sessionId, null);
    }, 550);
    return;
  }

  if (text.includes("reasoning effort probe")) {
    const thinkingLevel =
      session.thinkingByModel && typeof session.thinkingByModel === "object"
        ? session.thinkingByModel[session.model] || "off"
        : "off";
    emitMessageDelta(sessionId, "reasoning effort: " + thinkingLevel);
    recordHistoryMessage(sessionId, "assistant", "reasoning effort: " + thinkingLevel);
    emitContextMetrics(sessionId, 0.36);
    finishTurn(sessionId, null);
    return;
  }

  if (text.includes("message kind showcase")) {
    session.history.push(
      {
        id: "h-" + historyCounter++,
        message: {
          content: "please answer in Chinese",
          kind: "steering",
          role: "user",
        },
        type: "message",
      },
      {
        id: "h-" + historyCounter++,
        message: {
          content: "Finish the remaining plan tasks before stopping.",
          kind: "nudge",
          role: "user",
        },
        type: "message",
      },
      {
        id: "h-" + historyCounter++,
        message: {
          content: "Background task build-1 finished successfully.",
          kind: "signal",
          role: "user",
        },
        type: "message",
      },
    );
    emitMessageDelta(sessionId, "message kinds recorded");
    recordHistoryMessage(sessionId, "assistant", "message kinds recorded");
    emitContextMetrics(sessionId, 0.36);
    finishTurn(sessionId, null);
    return;
  }

  if (text.includes("retry 403 showcase") || text.includes("retry rejected showcase")) {
    const failureSummary = "API 错误 403 · aigateway.sunmi.com · Request-Id req-host-retry";
    const failureDetail = "API 错误 403: <html>forbidden</html>\\nHost: aigateway.sunmi.com\\nRequest-Id: req-host-retry";
    session.retry403ShowcaseAttempts = Number(session.retry403ShowcaseAttempts || 0) + 1;
    if (session.retry403ShowcaseAttempts === 1) {
      for (let attempt = 2; attempt <= 4; attempt += 1) {
        recordHistoryCustom(sessionId, "auto_retry_start", attempt);
      }
      recordHistoryCustom(sessionId, "auto_retry_end", 4);
      markLatestUserMessageFailed(sessionId);
      recordHistoryError(sessionId, failureSummary, failureDetail);
      finishTurn(sessionId, failureSummary);
      return;
    }
    emitMessageDelta(sessionId, "same session retry succeeded");
    recordHistoryMessage(sessionId, "assistant", "same session retry succeeded");
    emitContextMetrics(sessionId, 0.44);
    finishTurn(sessionId, null);
    return;
  }

  if (text.includes("resume card showcase")) {
    const tool = {
      args: { path: "README.md" },
      result: "[pending]",
      toolCallId: "resume-card-read-1",
      toolName: "read",
    };
    recordHistoryAssistantWithTools(
      sessionId,
      "I read the project overview before the connection failed.",
      [tool],
      "Read project overview",
    );
    markLatestUserMessageFailed(sessionId);
    recordHistoryError(
      sessionId,
      "连接中断 · 可继续",
      "The model stopped after all tool calls had already completed.",
    );
    // Recovery writes an unknown/pending tool result after its error record. The webview's
    // Resume scanner must cross that annotation boundary and use this actual transcript tail.
    recordHistoryToolResult(sessionId, tool);
    finishTurn(sessionId, "连接中断 · 可继续");
    return;
  }

  if (text.includes("tool icon showcase")) {
    emitMessageDelta(sessionId, "I prepared a built-in tool icon showcase.");
    send({
      assistantMessageEvent: {
        delta: "Review every built-in tool row and icon in one place.",
        kind: "thinking_delta",
      },
      assistantMessageId: ensurePendingAssistantMessageId(session),
      message: {},
      sessionId,
      type: "message_update",
    });
    const showcaseTools = buildToolIconShowcaseTools();
    for (const tool of showcaseTools) {
      emitCompletedTool(sessionId, tool);
    }
    emitTurnEnd(sessionId, {
      message: {},
      summaryTitle: "Built-in tool icons",
      toolResults: [],
      turnIndex: 1,
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
      assistantMessageId: ensurePendingAssistantMessageId(session),
      message: {},
      sessionId,
      type: "message_update",
    });
    const giantTools = buildGiantHistoryTools();
    for (const tool of giantTools) {
      emitCompletedTool(sessionId, tool);
    }
    emitTurnEnd(sessionId, {
      message: {},
      summaryTitle: "Giant history tool group",
      toolResults: [],
      turnIndex: 1,
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

  if (text.includes("edit display replay")) {
    emitMessageDelta(sessionId, "I prepared single-file and batch edit cards.");
    send({
      assistantMessageEvent: {
        delta: "Keep the single-file rich card for one file and the list card for many files.",
        kind: "thinking_delta",
      },
      assistantMessageId: ensurePendingAssistantMessageId(session),
      message: {},
      sessionId,
      type: "message_update",
    });
    const displayTools = buildEditDisplayReplayTools();
    for (const tool of displayTools) {
      emitCompletedTool(sessionId, tool);
    }
    emitTurnEnd(sessionId, {
      message: {},
      summaryTitle: "Edit display replay",
      toolResults: [],
      turnIndex: 1,
    });
    emitContextMetrics(sessionId, 0.35);
    recordHistoryAssistantWithTools(
      sessionId,
      "I prepared single-file and batch edit cards.",
      displayTools,
      "Edit display replay",
    );
    for (const tool of displayTools) {
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
      assistantMessageId: ensurePendingAssistantMessageId(session),
      message: {},
      sessionId,
      type: "message_update",
    });
    const readTool = {
      args: { path: readPath },
      display: { file: readPath, kind: "file" },
      result: "fake file contents for transcript ui showcase",
      toolCallId: "tc-transcript-read",
      toolName: "read",
    };
    emitCompletedTool(sessionId, readTool);
    const bashTool = {
      args: { command: "git status --short" },
      result: "M src/app.tsx\\n M README.md\\n?? plans/transcript-ui-showcase.plan.md",
      toolCallId: "tc-transcript-bash",
      toolName: "bash",
    };
    emitCompletedTool(sessionId, bashTool);
    const webSearchTool = {
      args: { query: "vscode chat thinking collapsible" },
      result:
        "Found 3 results.\\n- vscode/chatThinkingContentPart.ts\\n- chatCollapsibleContentPart.ts\\n- chatToolInvocationPart.ts",
      toolCallId: "tc-transcript-web-search",
      toolName: "web_search",
    };
    emitCompletedTool(sessionId, webSearchTool);
    const planPath = path.join(process.cwd(), "plans", "transcript-ui-showcase.plan.md");
    fs.mkdirSync(path.dirname(planPath), { recursive: true });
    fs.writeFileSync(planPath, ${transcriptPlanMarkdown}, "utf8");
    session.planId = "transcript-ui-showcase";
    session.planPath = planPath;
    session.planState = "planning";
    const planTodos = [
      { id: "t1", content: "Read the file", status: "completed" },
      { id: "t2", content: "Render the transcript UI", status: "in_progress" },
      { id: "t3", content: "Verify the screenshot crop", status: "pending" },
      { id: "t4", content: "Review the merged plan card", status: "pending" },
    ];
    const createPlanTool = {
      args: {
        draft: "Refresh the transcript UI and keep the plan card stable.",
        goal: "Transcript UI Showcase",
        todos: planTodos,
      },
      display: { kind: "plan", plan: planPath },
      result: JSON.stringify({
        path: planPath,
        plan_id: session.planId,
        state: "planning",
      }),
      toolCallId: "tc-transcript-create-plan",
      toolName: "create_plan",
    };
    send({
      args: createPlanTool.args,
      sessionId,
      toolCallId: createPlanTool.toolCallId,
      toolName: createPlanTool.toolName,
      type: "tool_execution_start",
    });
    emitPlanEvent(sessionId, "plan.create");
    session.planTodos = planTodos;
    send({
      planId: session.planId,
      sessionId,
      todos: planTodos,
      type: "plan.todos",
    });
    send({
      display: createPlanTool.display,
      isError: false,
      result: createPlanTool.result,
      sessionId,
      toolCallId: createPlanTool.toolCallId,
      toolName: createPlanTool.toolName,
      type: "tool_execution_end",
    });
    emitCustomPlanEvent(sessionId, "plan.review.warning", {
      reason: "rounds_exhausted",
    });
    ensureSession(sessionId).title = "Transcript UI Showcase";
    send({
      sessionId,
      title: "Transcript UI Showcase",
      type: "session.title_updated",
    });
    emitTurnEnd(sessionId, {
      message: {},
      summaryTitle: "Reviewed 1 file",
      toolResults: [],
      turnIndex: 1,
    });
    const transcriptTools = [readTool, bashTool, webSearchTool, createPlanTool];
    const finishTranscriptTurn = () => {
      emitContextMetrics(sessionId, 0.55);
      recordHistoryAssistantWithTools(
        sessionId,
        "I will read the file and refresh the plan.",
        transcriptTools,
        "Reviewed 1 file",
      );
      for (const tool of transcriptTools) {
        recordHistoryToolResult(sessionId, tool);
      }
      finishTurn(sessionId, null);
    };
    const finishDelayMs = text.includes("switch back order")
      ? Math.max(transcriptProgressDelayMs, 9000)
      : transcriptProgressDelayMs;
    if (finishDelayMs > 0) {
      setTimeout(finishTranscriptTurn, finishDelayMs);
    } else {
      finishTranscriptTurn();
    }
    return;
  }

  if (text.includes("plan tool ux")) {
    emitMessageDelta(sessionId, "I refreshed the plan card UX.");
    send({
      assistantMessageEvent: {
        delta: "Keep only one Creating plan header while the plan card reflects the tool state.",
        kind: "thinking_delta",
      },
      assistantMessageId: ensurePendingAssistantMessageId(session),
      message: {},
      sessionId,
      type: "message_update",
    });
    const planPath = path.join(process.cwd(), "plans", "plan-tool-ux.plan.md");
    fs.mkdirSync(path.dirname(planPath), { recursive: true });
    fs.writeFileSync(
      planPath,
      ${planToolUxMarkdown},
      "utf8",
    );
    session.planId = "plan-tool-ux";
    session.planPath = planPath;
    session.planState = "planning";
    emitPlanEvent(sessionId, "plan.create");
    const planTodos = [
      { id: "pt-1", content: "Render the plan card", status: "completed" },
      { id: "pt-2", content: "Hide duplicate plan rows", status: "in_progress" },
      { id: "pt-3", content: "Verify sticky history", status: "pending" },
    ];
    session.planTodos = planTodos;
    send({
      planId: session.planId,
      sessionId,
      todos: planTodos,
      type: "plan.todos",
    });
    send({
      args: { plan_id: session.planId },
      sessionId,
      toolCallId: "tc-plan-tool-ux",
      toolName: "update_plan",
      type: "tool_execution_start",
    });
    emitTurnEnd(sessionId, {
      message: {},
      summaryTitle: "Creating plan",
      toolResults: [],
      turnIndex: 1,
    });
    const completePlanTool = () => {
      send({
        display: { kind: "plan", plan: planPath },
        isError: false,
        result: "Updated plan plan-tool-ux",
        sessionId,
        toolCallId: "tc-plan-tool-ux",
        toolName: "update_plan",
        type: "tool_execution_end",
      });
    };
    const finishPlanToolTurn = () => {
      emitContextMetrics(sessionId, 0.52);
      recordHistoryMessage(sessionId, "assistant", "I refreshed the plan card UX.");
      finishTurn(sessionId, null);
    };
    if (transcriptProgressDelayMs > 0) {
      setTimeout(completePlanTool, Math.min(Math.max(transcriptProgressDelayMs - 100, 0), 400));
      setTimeout(finishPlanToolTurn, transcriptProgressDelayMs);
    } else {
      completePlanTool();
      finishPlanToolTurn();
    }
    return;
  }

  if (text.includes("plan replay")) {
    session.planId = "history-plan";
    session.planPath = path.join(process.cwd(), "plans", "history-plan.plan.md");
    fs.mkdirSync(path.dirname(session.planPath), { recursive: true });
    fs.writeFileSync(
      session.planPath,
      "---\\ngoal: Replay the plan review and verify history\\noverview: Confirm the merged plan card after reload.\\n---\\n\\n# History plan\\n\\n- Review\\n- Verify\\n",
      "utf8",
    );
    session.planState = "planning";
    const replayTodos = [
      { id: "rp-1", content: "Review the plan", status: "completed" },
      { id: "rp-2", content: "Verify the plan", status: "in_progress" },
      { id: "rp-3", content: "Confirm the merged card", status: "pending" },
    ];
    const createPlanTool = {
      args: {
        draft: "Replay the review and verify history without drifting the plan card.",
        goal: "Replay the plan review and verify history",
        todos: replayTodos,
      },
      display: { kind: "plan", plan: session.planPath },
      result: JSON.stringify({
        path: session.planPath,
        plan_id: session.planId,
        state: "planning",
      }),
      toolCallId: "tc-plan-replay-create",
      toolName: "create_plan",
    };
    send({
      args: createPlanTool.args,
      sessionId,
      toolCallId: createPlanTool.toolCallId,
      toolName: createPlanTool.toolName,
      type: "tool_execution_start",
    });
    emitPlanEvent(sessionId, "plan.create");
    send({
      display: createPlanTool.display,
      isError: false,
      result: createPlanTool.result,
      sessionId,
      toolCallId: createPlanTool.toolCallId,
      toolName: createPlanTool.toolName,
      type: "tool_execution_end",
    });
    session.planState = "executing";
    emitPlanEvent(sessionId, "plan.build");
    session.planTodos = replayTodos;
    send({
      planId: session.planId,
      sessionId,
      todos: replayTodos,
      type: "plan.todos",
    });
    session.history.push({
      event: "plan.todos",
      id: \`h-\${historyCounter++}\`,
      plan_id: session.planId,
      todos: replayTodos,
      type: "custom",
    });
    emitCustomPlanEvent(sessionId, "plan.review", { summary: "looks good" });
    emitCustomPlanEvent(sessionId, "plan.verify", { verdict: "pass" });
    session.planState = "pending";
    emitPlanEvent(sessionId, "plan.pending");
    emitMessageDelta(sessionId, "I replayed the plan review and verify history.");
    recordHistoryAssistantWithTools(
      sessionId,
      "I replayed the plan review and verify history.",
      [createPlanTool],
      "Replayed the plan review and verify history",
    );
    recordHistoryToolResult(sessionId, createPlanTool);
    emitContextMetrics(sessionId, 0.62);
    finishTurn(sessionId, null);
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

function handleResume(frame) {
  const sessionId = frame.sessionId || activeSessionId || createSession();
  const session = touchSession(ensureSession(sessionId));
  let previousUser = null;
  for (let index = session.history.length - 1; index >= 0; index -= 1) {
    const entry = session.history[index];
    if (entry?.type === "message" && entry?.message?.role === "user") {
      previousUser = entry;
      break;
    }
  }
  const content = previousUser?.message?.content;
  if (typeof content !== "string" || !content.trim()) {
    send({
      error: "resume_without_user_prompt",
      id: frame.id,
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
  if (content.includes("resume card showcase")) {
    emitMessageDelta(sessionId, "same session Resume continued from the healed tool result");
    recordHistoryMessage(
      sessionId,
      "assistant",
      "same session Resume continued from the healed tool result",
    );
    emitContextMetrics(sessionId, 0.44);
    finishTurn(sessionId, null);
    return;
  }
  finishTurn(sessionId, null);
}

function handleRetry(frame) {
  const sessionId = frame.sessionId || activeSessionId || createSession();
  const copied = copyUserMessageForward(sessionId, frame.messageId);
  const content = copied?.message?.content;
  if (typeof content !== "string" || !content.trim()) {
    send({
      error: "retry_target_stale",
      id: frame.id,
      sessionId,
      success: false,
      type: "response",
    });
    return;
  }
  if (content.includes("retry rejected showcase")) {
    send({
      error: "retry_target_stale",
      id: frame.id,
      sessionId,
      success: false,
      type: "response",
    });
    return;
  }
  // The copy is already durable; use the normal prompt simulation only to execute the next
  // model turn, never to synthesize another user row.
  handlePrompt({ ...frame, params: {}, resume: true, sessionId, text: content });
}

function handleControlResponse(frame) {
  if (!pendingApproval || frame.requestId !== pendingApproval.requestId) {
    return;
  }

  const pending = pendingApproval;
  pendingApproval = null;
  persistPendingApproval();
  const sessionId = pending.sessionId;
  const result =
    frame.payload && frame.payload.result && typeof frame.payload.result === "object"
      ? frame.payload.result
      : { answers: [], cancelled: false };
  if (pending.kind === "answer-card") {
    const tool = {
      args: { questions: pending.request.questions },
      result,
      toolCallId: pending.request.toolCallId,
      toolName: "ask_question",
    };
    for (const entry of ensureSession(sessionId).history) {
      if (entry.message && entry.message.tool_call_id === tool.toolCallId && entry.message.content === "[pending]") {
        entry.message.superseded = true;
      }
    }
    emitCompletedTool(sessionId, tool);
    clearPendingAssistantMessageId(sessionId);
    emitMessageDelta(sessionId, "Recorded your answer.");
    emitTurnEnd(sessionId, {
      message: {},
      summaryTitle: "Asked question",
      toolResults: [],
      turnIndex: 1,
    });
    recordHistoryToolResult(sessionId, tool);
    recordHistoryMessage(sessionId, "assistant", "Recorded your answer.");
    emitContextMetrics(sessionId, 0.49);
    finishTurn(sessionId, null);
    return;
  }

  const answers = Array.isArray(result.answers) ? result.answers : [];
  const firstAnswer = answers[0] || {};
  const pickedApprove = Array.isArray(firstAnswer.optionIds) && firstAnswer.optionIds.includes("approve");

  if (pickedApprove) {
    send({
      args: { path: editFilePath },
      sessionId,
      toolCallId: "toolu_01AbC",
      toolName: "write",
      type: "tool_execution_start",
    });
    setTimeout(() => {
      fs.writeFileSync(editFilePath, "after\\n", "utf8");
      send({
        display: {
          added: 1,
          diff: createReplacementDiff(["before"], ["after"]),
          file: editFilePath,
          kind: "file",
          removed: 1,
        },
        isError: false,
        result: { ok: true },
        sessionId,
        toolCallId: "toolu_01AbC",
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

  const settleInterrupt = () => {
    if (pendingInterrupt) {
      clearTimeout(pendingInterrupt);
      pendingInterrupt = null;
    }
    if (pendingPlanCompletion) {
      clearTimeout(pendingPlanCompletion);
      pendingPlanCompletion = null;
    }

    const session = ensureSession(sessionId);
    if (session.busy) {
      if (session.planState === "executing") {
        session.planState = "pending";
        emitPlanEvent(sessionId, "plan.pending");
      }
      recordHistoryCustom(sessionId, "agent.interrupted");
      send({
        sessionId,
        type: "agent_interrupted",
      });
      finishTurn(sessionId, "interrupted");
    }
  };
  if (interruptDelayMs > 0) {
    setTimeout(settleInterrupt, interruptDelayMs);
  } else {
    settleInterrupt();
  }
}

function handleCommand(frame) {
  switch (frame.type) {
    case "control_request":
      if (frame.subtype === "initialize") {
        const sessionId = activeSessionId || createSession();
        const respondToInitialize = () => {
          send({
            payload: {
              attachmentRoot: ATTACHMENT_ROOT,
              capabilities: [
                "prompt",
                "ask_question",
                "ingest_attachment",
                "retain_attachment_leases",
                "cache_attachment_thumbnail",
                "discard_detached_session",
                "new_session",
                "switch_session",
                "list_sessions",
                "get_state",
                "list_checkpoints",
                "close_session",
                "interrupt",
                "follow_up",
                "list_models",
                "upsert_model",
                "remove_model",
                "set_provider_key",
                "list_provider_keys",
                "set_model",
                "set_thinking_level",
                "set_context_window",
                "set_plan_mode",
              ],
              protocolVersion: 2,
              serverVersion,
              sessionId,
            },
            requestId: frame.requestId,
            sessionId,
            type: "control_response",
          });
          if (pendingApproval && pendingApproval.kind === "answer-card") {
            const requestId = \`ask-resumed-\${pendingApproval.sessionId}-\${Date.now()}\`;
            const request = {
              ...pendingApproval.request,
              requestId,
              responseEvent: \`plan.ask_question.response.\${requestId}\`,
            };
            pendingApproval = { ...pendingApproval, request, requestId };
            persistPendingApproval();
            setTimeout(() => {
              send({
                payload: request,
                requestId,
                sessionId: pendingApproval.sessionId,
                subtype: "ask_question",
                type: "control_request",
              });
            }, 0);
          }
        };
        if (handshakeDelayMs > 0) {
          setTimeout(respondToInitialize, handshakeDelayMs);
        } else {
          respondToInitialize();
        }
      }
      break;
    case "control_response":
      handleControlResponse(frame);
      break;
    case "new_session": {
      const sessionId = createSession();
      if (!frame.params || frame.params.detached !== true) {
        activeSessionId = sessionId;
      }
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
            title: session.title ?? null,
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
          activePlan: activePlanForSession(session),
          agentMode: session.agentMode || "chat",
          model: session.model,
          planTodos: session.planTodos ?? [],
          sessionId,
          sessionKey: session.sessionKey,
          sessionTodos: session.sessionTodos ?? [],
          workspaceMode: session.mode,
          thinkingLevel:
            session.thinkingByModel && typeof session.thinkingByModel === "object"
              ? session.thinkingByModel[session.model] || null
              : null,
        },
        sessionId,
        success: true,
        type: "response",
      });
      break;
    }
    case "list_checkpoints": {
      const sessionId = frame.sessionId || activeSessionId;
      send({
        id: frame.id,
        payload: {
          checkpoints: [],
          sessionId,
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
      attachmentLeases.delete(frame.sessionId);
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
      if (bootstrapListModelsFailures > 0) {
        bootstrapListModelsFailures -= 1;
        send({
          error: "simulated list_models bootstrap failure",
          id: frame.id,
          sessionId: activeSessionId,
          success: false,
          type: "response",
        });
        break;
      }
      send({
        id: frame.id,
        payload: {
          models: listModelViews(),
        },
        sessionId: activeSessionId,
        success: true,
        type: "response",
      });
      break;
    case "upsert_model": {
      const normalized = normalizeModelEntry(frame.model, "user");
      userModels.set(normalized.id, normalized);
      persistUserModels();
      const warnings = collectModelWarnings(normalized.api, normalized.thinkingFormat);
      send({
        id: frame.id,
        payload: {
          model: normalized,
          warnings,
        },
        success: true,
        type: "response",
      });
      break;
    }
    case "remove_model":
      userModels.delete(frame.modelId);
      send({
        id: frame.id,
        payload: {
          modelId: frame.modelId,
          removed: true,
        },
        success: true,
        type: "response",
      });
      break;
    case "set_provider_key": {
      const entry = {
        keyPresent: typeof frame.value === "string" && frame.value.trim().length > 0,
        provider:
          listModelViews().find((model) => model.apiKeyEnv === frame.envName)?.provider || "openai",
      };
      providerKeys.set(frame.envName, entry);
      for (const model of userModels.values()) {
        if (model.apiKeyEnv === frame.envName) model.keyPresent = entry.keyPresent;
      }
      persistUserModels();
      send({
        id: frame.id,
        payload: {
          envName: frame.envName,
          keyPresent: entry.keyPresent,
        },
        success: true,
        type: "response",
      });
      break;
    }
    case "list_provider_keys":
      send({
        id: frame.id,
        payload: {
          keys: listProviderKeyViews(),
        },
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
    case "set_thinking_level": {
      const sessionId = frame.sessionId || activeSessionId || createSession();
      const session = touchSession(ensureSession(sessionId));
      const modelId =
        typeof frame.model === "string" && frame.model.length > 0 ? frame.model : session.model;
      const level = normalizeReasoningLevel(frame.level);
      session.thinkingByModel = session.thinkingByModel || {};
      session.thinkingByModel[modelId] = level || "off";
      modelPreferences.set(modelId, {
        ...modelPreferences.get(modelId),
        reasoning: session.thinkingByModel[modelId],
      });
      persistModelPreferences();
      const userModel = userModels.get(modelId);
      if (userModel) {
        userModel.selectedReasoningLevel = session.thinkingByModel[modelId];
        persistUserModels();
      } else if (level) {
        for (const model of userModels.values()) {
          model.selectedReasoningLevel = level;
        }
        persistUserModels();
      }
          send({
        id: frame.id,
        payload: { model: modelId, sessionId, thinkingLevel: session.thinkingByModel[modelId] },
        sessionId,
        success: true,
        type: "response",
      });
      break;
    }
    case "set_context_window": {
      const modelId = typeof frame.model === "string" ? frame.model : "";
      const contextWindow = frame.contextWindow;
      const model = listModelViews().find((candidate) => candidate.id === modelId);
      if (!model || !model.contextWindowOptions.includes(contextWindow)) {
        send({ id: frame.id, error: "invalid_context_window", success: false, type: "response" });
        break;
      }
      modelPreferences.set(modelId, {
        ...modelPreferences.get(modelId),
        contextWindow,
      });
      persistModelPreferences();
      send({
        id: frame.id,
        payload: { contextWindow, model: modelId, sessionId: frame.sessionId || activeSessionId },
        sessionId: frame.sessionId || activeSessionId,
        success: true,
        type: "response",
      });
      break;
    }
    case "set_plan_mode": {
      const sessionId = frame.sessionId || activeSessionId || createSession();
      const session = touchSession(ensureSession(sessionId));
      if (frame.action === "enter") {
        session.agentMode = "plan";
        session.planState = "planning";
        session.planId = frame.planId || session.planId || "fake-plan";
        session.planPath = path.join(process.cwd(), "plans", \`\${session.planId}.plan.md\`);
        fs.mkdirSync(path.dirname(session.planPath), { recursive: true });
        fs.writeFileSync(session.planPath, "# Fake plan\\n\\n- Step 1\\n", "utf8");
        send({
          id: frame.id,
          payload: planControlPayload(session),
          sessionId,
          success: true,
          type: "response",
        });
        emitAgentModeChanged(sessionId);
        break;
      }
      if (frame.action === "exit") {
        session.agentMode = "chat";
        send({
          id: frame.id,
          payload: planControlPayload(session),
          sessionId,
          success: true,
          type: "response",
        });
        emitAgentModeChanged(sessionId);
        break;
      }

      const resumesPendingPlan = session.planState === "pending";
      session.planId = frame.planId || session.planId || "fake-plan";
      session.planPath = session.planPath || path.join(process.cwd(), "plans", \`\${session.planId}.plan.md\`);
      fs.mkdirSync(path.dirname(session.planPath), { recursive: true });
      fs.writeFileSync(session.planPath, "# Fake plan\\n\\n- Step 1\\n- Build\\n", "utf8");
      session.planState = "executing";
      session.agentMode = "chat";
      // Mirror production ordering: persist the user-visible kickoff before
      // acknowledging build, so the immediate history refresh can render it.
      session.history.push({
        id: "h-" + historyCounter++,
        message: {
          content: "start building " + session.planPath,
          kind: "plan_build",
          role: "user",
        },
        type: "message",
      });
      send({
        id: frame.id,
        payload: planControlPayload(session),
        sessionId,
        success: true,
        type: "response",
      });
      startTurn(sessionId);
      emitPlanEvent(sessionId, "plan.build");
      emitAgentModeChanged(sessionId);
      const finishPlanRun = () => {
        pendingPlanCompletion = null;
        if (session.planId === "fake-plan-complete") {
          session.planState = "completed";
          emitPlanEvent(sessionId, "plan.complete");
        }
        finishTurn(sessionId, null);
      };
      const completionDelayMs =
        session.planId === "fake-plan-interrupt" && !resumesPendingPlan
          ? 5_000
          : resumesPendingPlan
            ? 250
            : 10;
      pendingPlanCompletion = setTimeout(finishPlanRun, completionDelayMs);
      break;
    }
    case "retain_attachment_leases": {
      const sessionId = frame.sessionId;
      if (!sessions.has(sessionId)) {
        send({ error: "unknown_session", id: frame.id, sessionId, success: false, type: "response" });
        break;
      }
      const attachments = frame.params && Array.isArray(frame.params.attachments)
        ? frame.params.attachments
        : [];
      const retainedShas = [...new Set(attachments.flatMap((attachment) => [
        attachment && attachment.blobSha,
        attachment && attachment.providerSha,
      ]).filter((sha) => typeof sha === "string"))].sort();
      const missing = retainedShas.find((sha) => !fs.existsSync(path.join(BLOBS_DIR, sha)));
      if (missing) {
        send({ error: \`missing_attachment_blob: \${missing}\`, id: frame.id, sessionId, success: false, type: "response" });
        break;
      }
      attachmentLeases.set(sessionId, new Set([
        ...(attachmentLeases.get(sessionId) || []),
        ...retainedShas,
      ]));
      send({
        id: frame.id,
        payload: { retainedShas },
        sessionId,
        success: true,
        type: "response",
      });
      break;
    }
    case "discard_detached_session": {
      const discarded = sessions.delete(frame.sessionId);
      attachmentLeases.delete(frame.sessionId);
      send({
        id: frame.id,
        payload: { discarded, sessionId: frame.sessionId },
        sessionId: frame.sessionId,
        success: true,
        type: "response",
      });
      break;
    }
    case "ingest_attachment":
      handleIngestAttachment(frame);
      break;
    case "cache_attachment_thumbnail":
      handleCacheAttachmentThumbnail(frame);
      break;
    case "prompt":
    case "follow_up":
      handlePrompt(frame);
      break;
    case "resume":
      handleResume(frame);
      break;
    case "retry":
      handleRetry(frame);
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

export async function createHostE2eFixture(
  options: HostE2eFixtureOptions = {},
): Promise<HostE2eFixture> {
  const rootDir = await mkdtemp(
    path.join(os.tmpdir(), "tomcat-vscode-ext-host-"),
  );
  const workspaceDir = path.join(rootDir, "workspace");
  const fakeServePath = path.join(rootDir, "fake-tomcat.js");
  const editFilePath = path.join(rootDir, "edit-target.txt");
  const setupMarkerPath = path.join(rootDir, "setup", "ready");
  const transientFailureMarkerPath = path.join(rootDir, "setup", "transient-failures");
  const attachmentRootDir = path.join(rootDir, "attachments");

  await mkdir(workspaceDir, { recursive: true });
  // Created up front so the host can grant the webview access to a path that exists;
  // an ingest that happens to be the first write should not be what creates it.
  await mkdir(path.join(attachmentRootDir, "blobs"), { recursive: true });
  await mkdir(path.join(attachmentRootDir, "thumbs"), { recursive: true });
  await writeFile(editFilePath, "before\n", "utf8");
  await writeFile(
    fakeServePath,
    buildFakeServeSource(editFilePath, options, setupMarkerPath),
    "utf8",
  );
  await chmod(fakeServePath, 0o755);

  return {
    attachmentRootDir,
    async cleanup() {
      await rm(rootDir, { force: true, recursive: true });
    },
    editFilePath,
    env: {
      ...process.env,
      TOMCAT_VSCODE_TEST_ATTACHMENT_ROOT: attachmentRootDir,
      TOMCAT_VSCODE_TEST_BOOTSTRAP_LIST_MODELS_FAILURES: String(
        options.bootstrapListModelsFailures ?? 0,
      ),
      TOMCAT_VSCODE_TEST_DEFAULT_CWD: workspaceDir,
      TOMCAT_VSCODE_TEST_EDIT_FILE: editFilePath,
      TOMCAT_VSCODE_TEST_HANDSHAKE_DELAY_MS: String(
        options.handshakeDelayMs ?? 0,
      ),
      TOMCAT_VSCODE_TEST_INTERRUPT_DELAY_MS: String(
        options.interruptDelayMs ?? 150,
      ),
      TOMCAT_VSCODE_TEST_PATH: fakeServePath,
      TOMCAT_VSCODE_TEST_REQUIRE_INIT: options.requireInit ? "1" : "0",
      TOMCAT_VSCODE_TEST_SETUP_MARKER: setupMarkerPath,
      TOMCAT_VSCODE_TEST_SUPPRESS_EXIT_PROMPT: "1",
      TOMCAT_VSCODE_TEST_TRANSIENT_FAILURE_MARKER: transientFailureMarkerPath,
      TOMCAT_VSCODE_TEST_TRANSIENT_SERVE_FAILURES: String(
        options.transientServeFailures ?? 0,
      ),
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
    "/Applications/Visual Studio Code.app/Contents/MacOS/Code"
  );
}
