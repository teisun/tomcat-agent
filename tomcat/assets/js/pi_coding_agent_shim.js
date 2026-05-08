// pi_coding_agent_shim.js — @mariozechner/pi-coding-agent globalThis shim for WasmEdge QuickJS script mode.
// Ported from pi_agent_rust default_virtual_modules(); see task-05d-compat-research.md Part 4.
(function () {
  'use strict';

  var VERSION = "0.0.0";
  var DEFAULT_MAX_LINES = 2000;
  var DEFAULT_MAX_BYTES = 50 * 1024;

  function jsBytes(value) { return String(value == null ? "" : value).length; }

  function formatSize(bytes) {
    var b = Number(bytes || 0);
    var KB = 1024, MB = 1024 * 1024;
    if (b >= MB) return (b / MB).toFixed(1) + "MB";
    if (b >= KB) return (b / KB).toFixed(1) + "KB";
    return Math.trunc(b) + "B";
  }

  function truncateHead(text, opts) {
    var raw = String(text == null ? "" : text);
    opts = opts || {};
    var maxLines = Number(opts.maxLines == null ? DEFAULT_MAX_LINES : opts.maxLines);
    var maxBytes = Number(opts.maxBytes == null ? DEFAULT_MAX_BYTES : opts.maxBytes);
    var lines = raw.split("\n");
    var totalLines = lines.length, totalBytes = jsBytes(raw);
    var out = [], outBytes = 0, truncatedBy = null;
    for (var i = 0; i < lines.length; i++) {
      if (out.length >= maxLines) { truncatedBy = "lines"; break; }
      var candidate = out.length ? "\n" + lines[i] : lines[i];
      var candidateBytes = jsBytes(candidate);
      if (outBytes + candidateBytes > maxBytes) { truncatedBy = "bytes"; break; }
      out.push(lines[i]);
      outBytes += candidateBytes;
    }
    var content = out.join("\n");
    return {
      content: content, truncated: truncatedBy != null, truncatedBy: truncatedBy,
      totalLines: totalLines, totalBytes: totalBytes,
      outputLines: out.length, outputBytes: jsBytes(content),
      lastLinePartial: false, firstLineExceedsLimit: false,
      maxLines: maxLines, maxBytes: maxBytes
    };
  }

  function truncateTail(text, opts) {
    var raw = String(text == null ? "" : text);
    opts = opts || {};
    var maxLines = Number(opts.maxLines == null ? DEFAULT_MAX_LINES : opts.maxLines);
    var maxBytes = Number(opts.maxBytes == null ? DEFAULT_MAX_BYTES : opts.maxBytes);
    var lines = raw.split("\n");
    var totalLines = lines.length, totalBytes = jsBytes(raw);
    var out = [], outBytes = 0, truncatedBy = null;
    for (var i = lines.length - 1; i >= 0; i--) {
      if (out.length >= maxLines) { truncatedBy = "lines"; break; }
      var candidate = out.length ? lines[i] + "\n" : lines[i];
      var candidateBytes = jsBytes(candidate);
      if (outBytes + candidateBytes > maxBytes) { truncatedBy = "bytes"; break; }
      out.unshift(lines[i]);
      outBytes += candidateBytes;
    }
    var content = out.join("\n");
    return {
      content: content, truncated: truncatedBy != null, truncatedBy: truncatedBy,
      totalLines: totalLines, totalBytes: totalBytes,
      outputLines: out.length, outputBytes: jsBytes(content),
      lastLinePartial: false, firstLineExceedsLimit: false,
      maxLines: maxLines, maxBytes: maxBytes
    };
  }

  function parseSessionEntries(text) {
    var raw = String(text == null ? "" : text);
    var out = [];
    var lines = raw.split(/\r?\n/);
    for (var i = 0; i < lines.length; i++) {
      var trimmed = lines[i].trim();
      if (!trimmed) continue;
      try { out.push(JSON.parse(trimmed)); } catch (_) {}
    }
    return out;
  }

  function convertToLlm(entries) { return entries; }

  function serializeConversation(entries) {
    try { return JSON.stringify(entries || []); } catch (_) { return String(entries || ""); }
  }

  function parseFrontmatter(text) {
    var raw = String(text == null ? "" : text);
    if (raw.indexOf("---") !== 0) return { frontmatter: {}, body: raw };
    var end = raw.indexOf("\n---", 3);
    if (end === -1) return { frontmatter: {}, body: raw };
    var header = raw.slice(3, end).trim();
    var body = raw.slice(end + 4).replace(/^\n/, "");
    var frontmatter = {};
    var hlines = header.split(/\r?\n/);
    for (var i = 0; i < hlines.length; i++) {
      var idx = hlines[i].indexOf(":");
      if (idx === -1) continue;
      var key = hlines[i].slice(0, idx).trim();
      var val = hlines[i].slice(idx + 1).trim();
      if (key) frontmatter[key] = val;
    }
    return { frontmatter: frontmatter, body: body };
  }

  function getMarkdownTheme() { return {}; }
  function getSettingsListTheme() { return {}; }
  function getSelectListTheme() { return {}; }
  function copyToClipboard() {}
  function highlightCode(code) { return String(code == null ? "" : code); }

  function getLanguageFromPath(filePath) {
    var ext = String(filePath == null ? "" : filePath).split(".").pop() || "";
    var map = { ts: "typescript", js: "javascript", py: "python", rs: "rust", go: "go", md: "markdown", json: "json", html: "html", css: "css", sh: "bash" };
    return map[ext] || ext;
  }

  function isBashToolResult(result) {
    return result && typeof result === "object" && result.name === "bash";
  }

  function loadSkills() { return Promise.resolve([]); }

  function truncateToVisualLines(text, maxLines) {
    var raw = String(text == null ? "" : text);
    var lines = raw.split(/\r?\n/);
    maxLines = Number(maxLines == null ? DEFAULT_MAX_LINES : maxLines);
    if (!Number.isFinite(maxLines) || maxLines <= 0) return "";
    return lines.slice(0, Math.floor(maxLines)).join("\n");
  }

  function estimateTokens(input) {
    var raw = typeof input === "string" ? input : JSON.stringify(input || "");
    return Math.max(1, Math.ceil(String(raw).length / 4));
  }

  function isToolCallEventType(value) {
    var t = String((value && value.type) || value || "").toLowerCase();
    return t === "tool_call" || t === "tool-call" || t === "toolcall";
  }

  function getAgentDir() {
    return "/home/unknown/.pi/agent";
  }

  function keyHint(action, fallback) {
    var keyMap = { expandTools: "Ctrl+E", copy: "Ctrl+C", paste: "Ctrl+V", save: "Ctrl+S", quit: "Ctrl+Q", help: "?" };
    return keyMap[action] || fallback || action;
  }

  function compact() {
    return Promise.resolve({ summary: "Conversation summary placeholder", firstKeptEntryId: null, tokensBefore: 0, tokensAfter: 0 });
  }

  function DynamicBorder() {}
  function BorderedLoader() {}
  function CustomEditor() { this.value = ""; }
  CustomEditor.prototype.handleInput = function () {};
  CustomEditor.prototype.render = function () { return []; };

  function createBashTool() { return { name: "bash", label: "bash", description: "bash", parameters: { type: "object", properties: { command: { type: "string" } }, required: ["command"] }, execute: function () { return Promise.resolve({ content: [{ type: "text", text: "" }], details: {} }); } }; }
  function createReadTool() { return { name: "read", label: "read", description: "read", parameters: { type: "object", properties: { path: { type: "string" } }, required: ["path"] }, execute: function () { return Promise.resolve({ content: [{ type: "text", text: "" }], details: {} }); } }; }
  function createLsTool() { return { name: "ls", label: "ls", description: "ls", parameters: { type: "object", properties: { path: { type: "string" } }, required: ["path"] }, execute: function () { return Promise.resolve({ content: [{ type: "text", text: "" }], details: {} }); } }; }
  function createGrepTool() { return { name: "grep", label: "grep", description: "grep", parameters: { type: "object", properties: { pattern: { type: "string" } }, required: ["pattern"] }, execute: function () { return Promise.resolve({ content: [{ type: "text", text: "" }], details: {} }); } }; }
  function createWriteTool() { return { name: "write", label: "write", description: "write", parameters: { type: "object", properties: { path: { type: "string" }, content: { type: "string" } }, required: ["path", "content"] }, execute: function () { return Promise.resolve({ content: [{ type: "text", text: "" }], details: {} }); } }; }
  function createEditTool() { return { name: "edit", label: "edit", description: "edit", parameters: { type: "object", properties: { path: { type: "string" }, oldText: { type: "string" }, newText: { type: "string" } }, required: ["path", "oldText", "newText"] }, execute: function () { return Promise.resolve({ content: [{ type: "text", text: "" }], details: {} }); } }; }

  function AssistantMessageComponent(message, editable) { this.message = message; this.editable = !!editable; }
  AssistantMessageComponent.prototype.render = function () { return []; };

  function ToolExecutionComponent(toolName, args, opts, result, ui) { this.toolName = toolName; this.args = args; this.opts = opts || {}; this.result = result; this.ui = ui; }
  ToolExecutionComponent.prototype.render = function () { return []; };

  function UserMessageComponent(text) { this.text = text; }
  UserMessageComponent.prototype.render = function () { return []; };

  function SessionManager() {}
  SessionManager.inMemory = function () { return new SessionManager(); };
  SessionManager.prototype.getSessionFile = function () { return ""; };
  SessionManager.prototype.getSessionDir = function () { return ""; };
  SessionManager.prototype.getSessionId = function () { return ""; };

  function SettingsManager(cwd, agentDir) { this.cwd = String(cwd || ""); this.agentDir = String(agentDir || ""); }
  SettingsManager.create = function (cwd, agentDir) { return new SettingsManager(cwd, agentDir); };

  function DefaultResourceLoader(opts) { this.opts = opts || {}; }
  DefaultResourceLoader.prototype.reload = function () { return Promise.resolve(); };

  function AuthStorage() {}
  AuthStorage.load = function () { return new AuthStorage(); };
  AuthStorage.loadAsync = function () { return Promise.resolve(new AuthStorage()); };
  AuthStorage.prototype.resolveApiKey = function () { return undefined; };
  AuthStorage.prototype.get = function () { return undefined; };

  function withFileMutationQueue(fn) {
    return typeof fn === 'function' ? fn() : Promise.resolve();
  }

  function createAgentSession(opts) {
    opts = opts || {};
    var state = { id: String(opts.id || "session"), messages: Array.isArray(opts.messages) ? opts.messages.slice() : [] };
    return {
      id: state.id, messages: state.messages,
      append: function (entry) { state.messages.push(entry); },
      toJSON: function () { return { id: state.id, messages: state.messages.slice() }; }
    };
  }

  globalThis.__pi_coding_agent = {
    VERSION: VERSION, DEFAULT_MAX_LINES: DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES: DEFAULT_MAX_BYTES,
    formatSize: formatSize, truncateHead: truncateHead, truncateTail: truncateTail,
    parseSessionEntries: parseSessionEntries, convertToLlm: convertToLlm,
    serializeConversation: serializeConversation, parseFrontmatter: parseFrontmatter,
    getMarkdownTheme: getMarkdownTheme, getSettingsListTheme: getSettingsListTheme,
    getSelectListTheme: getSelectListTheme,
    DynamicBorder: DynamicBorder, BorderedLoader: BorderedLoader, CustomEditor: CustomEditor,
    createBashTool: createBashTool, createReadTool: createReadTool, createLsTool: createLsTool,
    createGrepTool: createGrepTool, createWriteTool: createWriteTool, createEditTool: createEditTool,
    copyToClipboard: copyToClipboard, getAgentDir: getAgentDir, keyHint: keyHint, compact: compact,
    withFileMutationQueue: withFileMutationQueue,
    AssistantMessageComponent: AssistantMessageComponent,
    ToolExecutionComponent: ToolExecutionComponent, UserMessageComponent: UserMessageComponent,
    SessionManager: SessionManager, SettingsManager: SettingsManager,
    DefaultResourceLoader: DefaultResourceLoader,
    highlightCode: highlightCode, getLanguageFromPath: getLanguageFromPath,
    isBashToolResult: isBashToolResult, loadSkills: loadSkills,
    truncateToVisualLines: truncateToVisualLines, estimateTokens: estimateTokens,
    isToolCallEventType: isToolCallEventType,
    AuthStorage: AuthStorage, createAgentSession: createAgentSession,
    "default": {
      VERSION: VERSION, DEFAULT_MAX_LINES: DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES: DEFAULT_MAX_BYTES,
      formatSize: formatSize, truncateHead: truncateHead, truncateTail: truncateTail,
      parseSessionEntries: parseSessionEntries, convertToLlm: convertToLlm,
      serializeConversation: serializeConversation, parseFrontmatter: parseFrontmatter,
      getMarkdownTheme: getMarkdownTheme, getSettingsListTheme: getSettingsListTheme,
      getSelectListTheme: getSelectListTheme,
      DynamicBorder: DynamicBorder, BorderedLoader: BorderedLoader, CustomEditor: CustomEditor,
      createBashTool: createBashTool, createReadTool: createReadTool, createLsTool: createLsTool,
      createGrepTool: createGrepTool, createWriteTool: createWriteTool, createEditTool: createEditTool,
      copyToClipboard: copyToClipboard, getAgentDir: getAgentDir, keyHint: keyHint, compact: compact,
      withFileMutationQueue: withFileMutationQueue,
      AssistantMessageComponent: AssistantMessageComponent,
      ToolExecutionComponent: ToolExecutionComponent, UserMessageComponent: UserMessageComponent,
      SessionManager: SessionManager, SettingsManager: SettingsManager,
      DefaultResourceLoader: DefaultResourceLoader,
      highlightCode: highlightCode, getLanguageFromPath: getLanguageFromPath,
      isBashToolResult: isBashToolResult, loadSkills: loadSkills,
      truncateToVisualLines: truncateToVisualLines, estimateTokens: estimateTokens,
      isToolCallEventType: isToolCallEventType,
      AuthStorage: AuthStorage, createAgentSession: createAgentSession
    }
  };
})();
