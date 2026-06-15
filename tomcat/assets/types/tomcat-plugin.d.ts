/**
 * Tomcat plugin authoring types.
 *
 * Runtime note:
 * - `pi` is injected by the Tomcat host at runtime.
 * - Plugin source files should NOT import `pi_bridge.js`.
 * - For JS projects, enable `// @ts-check` or add this file to `jsconfig.json`.
 * - For TS projects, include this file via `tsconfig.json` or a triple-slash reference.
 */

type TomcatJsonValue =
  | null
  | boolean
  | number
  | string
  | TomcatJsonValue[]
  | { [key: string]: TomcatJsonValue };

type TomcatRole = "system" | "user" | "assistant" | "tool" | string;

interface TomcatAnnotation {
  type?: string;
  [key: string]: TomcatJsonValue | undefined;
}

interface TomcatUrlCitationAnnotation extends TomcatAnnotation {
  type: "url_citation";
  title?: string | null;
  url?: string | null;
  snippet?: string | null;
  summary?: string | null;
  publish_time?: string | null;
  published_at?: string | null;
}

interface TomcatChatMessagePart {
  type?: string;
  text?: string;
  annotations?: TomcatAnnotation[];
  [key: string]: TomcatJsonValue | undefined;
}

interface TomcatChatMessage {
  role: TomcatRole;
  content?: string | TomcatChatMessagePart[] | null;
  annotations?: TomcatAnnotation[];
  name?: string;
  tool_call_id?: string;
  [key: string]: TomcatJsonValue | undefined;
}

interface TomcatToolDefinition {
  type?: string;
  name?: string;
  label?: string;
  description?: string;
  parameters?: TomcatJsonValue;
  [key: string]: TomcatJsonValue | undefined;
}

interface TomcatChatCompletionRequest {
  model?: string;
  messages: TomcatChatMessage[];
  tools?: TomcatToolDefinition[];
  [key: string]: TomcatJsonValue | undefined;
}

interface TomcatChatCompletionChoice {
  index?: number;
  message?: TomcatChatMessage;
  finish_reason?: string | null;
  [key: string]: TomcatJsonValue | undefined;
}

interface TomcatChatCompletionResponse {
  id?: string;
  model?: string;
  choices?: TomcatChatCompletionChoice[];
  usage?: TomcatJsonValue;
  [key: string]: TomcatJsonValue | undefined;
}

interface TomcatFetchRequest {
  method?: string;
  url: string;
  headers?: Record<string, string>;
  query?: Record<string, string | number | boolean | null | undefined>;
  body?: TomcatJsonValue;
  timeoutMs?: number;
  [key: string]: TomcatJsonValue | undefined;
}

interface TomcatFetchResponse {
  status: number;
  headers?: Record<string, string | string[]>;
  body: string;
  [key: string]: TomcatJsonValue | undefined;
}

interface TomcatToolExecutionResult {
  [key: string]: TomcatJsonValue | undefined;
}

interface TomcatRegisteredTool {
  name: string;
  label?: string;
  description?: string;
  parameters?: TomcatJsonValue;
  execute?: (
    toolCallId: string,
    params: TomcatJsonValue,
    input?: TomcatJsonValue,
    ctx?: TomcatPluginContext | null,
    rawCtx?: TomcatJsonValue
  ) => TomcatToolExecutionResult | Promise<TomcatToolExecutionResult>;
}

interface TomcatCommandOptions {
  description?: string;
  handler?: (args: string, ctx: TomcatPluginContext) => TomcatJsonValue | Promise<TomcatJsonValue>;
}

interface TomcatUiApi {
  notify(message: string, type?: string): unknown;
  select(title: string, options: string[]): string | null;
  confirm(title: string, message: string): boolean;
  input(title: string, placeholder?: string): string | null;
  setStatus(message: string, details?: string | null): unknown;
  editor(title: string, prefill?: string): string;
  setWidget(key: string, content: string): void;
  setFooter(factory: unknown): void;
  setHeader(factory: unknown): void;
  custom(factory: (...args: unknown[]) => unknown, options?: TomcatJsonValue): unknown;
}

interface TomcatSessionApi {
  getCurrent(): unknown;
  getMessages(cap?: number): unknown;
  sendMessage(message: string): unknown;
}

interface TomcatSessionManagerApi {
  getCurrent(): unknown;
  getBranch(fromId: string): unknown[];
  getLeafEntry(): unknown;
  getLeafId(): string | null;
  getEntry(id: string): unknown;
  getHeader(): unknown;
  getEntries(cap?: number): unknown[];
  getCwd(): string;
  getSessionDir(): string;
  getSessionId(): string;
  getSessionFile(): string;
  getTree(): unknown[];
  getLabel(): string | null;
}

interface TomcatModelRegistryApi {
  getAll(): unknown[];
  getAvailable(): unknown[];
  getError(): unknown;
  find(query: string): unknown;
  getApiKeyForProvider(provider: string): string;
}

interface TomcatPluginContext {
  cwd?: string;
  model?: TomcatJsonValue;
  hasUI?: boolean;
  isIdle(): boolean;
  abort(): unknown;
  hasPendingMessages(): boolean;
  shutdown(): unknown;
  getSystemPrompt(): string;
  getContextUsage(): TomcatJsonValue;
  compact(options?: TomcatJsonValue): unknown;
  ui: TomcatUiApi;
  sessionManager: TomcatSessionManagerApi;
  modelRegistry: TomcatModelRegistryApi;
}

interface TomcatPluginAPI {
  on(eventName: string, handler: (data: TomcatJsonValue, ctx: TomcatPluginContext) => void): number;
  off(eventName: string, handlerOrId: ((data: TomcatJsonValue, ctx: TomcatPluginContext) => void) | number): void;
  emit(eventName: string, payload?: TomcatJsonValue): unknown;
  once(eventName: string, handler: (data: TomcatJsonValue, ctx: TomcatPluginContext) => void): number;

  exec(command: string, args?: string[], options?: { cwd?: string }): Promise<{
    stdout?: string;
    stderr?: string;
    exitCode?: number;
  }>;
  readFile(path: string): Promise<string>;
  writeFile(path: string, content: string, options?: { overwrite?: boolean }): Promise<unknown>;
  editFile(path: string, edits: TomcatJsonValue): Promise<unknown>;

  registerTool(toolDef: TomcatRegisteredTool): unknown;
  unregisterTool(name: string): unknown;
  registerFunction(
    name: string,
    handler: (params: TomcatJsonValue, ctx?: TomcatPluginContext | null) => TomcatJsonValue | Promise<TomcatJsonValue>
  ): null;
  registerCommand(name: string, options?: TomcatCommandOptions): unknown;

  createChatCompletion(params: TomcatChatCompletionRequest): Promise<TomcatChatCompletionResponse>;
  fetch(params: TomcatFetchRequest): Promise<TomcatFetchResponse>;
  complete(
    prompt: string,
    options?: { messages?: TomcatChatMessage[] }
  ): Promise<string>;
  setModel(model: string): Promise<unknown>;
  getModel(): string | null;

  session: TomcatSessionApi;

  sendMessage(message: string, options?: TomcatJsonValue): unknown;
  sendUserMessage(content: string, options?: TomcatJsonValue): unknown;
  log(message: unknown): void;

  getActiveTools(): unknown;
  setActiveTools(toolNames: string[]): unknown;
  getAllTools(): unknown[];

  registerFlag(name: string, options?: { description?: string; type?: string }): unknown;
  getFlag(name: string): unknown;
  registerShortcut(key: string, options?: { description?: string }): unknown;

  getSessionName(): string;
  setSessionName(name: string): unknown;
  appendEntry(entry: TomcatJsonValue): unknown;
  setThinkingLevel(level: string): unknown;
}

declare global {
  const pi: TomcatPluginAPI;
}

export {};
