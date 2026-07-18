type CommandHandler = (...args: any[]) => any;
type Provider = { provideTextDocumentContent(uri: Uri): string };
type FileEntry = { text: string; type: number };

const commandHandlers = new Map<string, CommandHandler>();
const contentProviders = new Map<string, Provider>();
const files = new Map<string, FileEntry>();
const configuration = new Map<string, unknown>();
const configurationListeners = new Set<
  (event: { affectsConfiguration(section: string): boolean }) => void
>();
const textDocumentChangeListeners = new Set<(event: { document: TextDocument }) => void>();
const fileWatchers = new Set<{
  createListeners: Set<(uri: Uri) => void>;
  deleteListeners: Set<(uri: Uri) => void>;
  glob: string;
}>();
const textDocuments: TextDocument[] = [];

let quickPickHandler: ((items: QuickPickItem[]) => any) | undefined;
let inputBoxHandler: ((options: InputBoxOptions) => any) | undefined;
let infoMessageHandler: ((message: string, items: string[]) => any) | undefined;
let warningMessageHandler: ((message: string, items: string[]) => any) | undefined;
let openDialogHandler: ((options: unknown) => Uri[] | Promise<Uri[] | undefined> | undefined) | undefined;
let lastDiffCommand:
  | {
      original: Uri;
      modified: Uri;
      options?: unknown;
      title?: string;
    }
  | undefined;
let lastRevealRange:
  | {
      range: Range;
      revealType?: number;
    }
  | undefined;

export class Disposable {
  constructor(private readonly callback: () => void = () => undefined) {}

  dispose(): void {
    this.callback();
  }
}

function escapeRegex(value: string): string {
  return value.replace(/[|\\{}()[\]^$+?.]/g, "\\$&");
}

function normalizePath(value: string): string {
  return value.replace(/\\/g, "/");
}

function globToRegExp(glob: string): RegExp {
  const normalized = normalizePath(glob);
  let result = "^";
  for (let index = 0; index < normalized.length; index += 1) {
    const char = normalized[index];
    const next = normalized[index + 1];
    const nextNext = normalized[index + 2];
    if (char === "*" && next === "*" && nextNext === "/") {
      result += "(?:.*/)?";
      index += 2;
      continue;
    }
    if (char === "*" && next === "*") {
      result += ".*";
      index += 1;
      continue;
    }
    if (char === "*") {
      result += "[^/]*";
      continue;
    }
    if (char === "?") {
      result += ".";
      continue;
    }
    result += escapeRegex(char);
  }
  result += "$";
  return new RegExp(result);
}

function matchesAnyGlob(candidatePath: string, globs: readonly string[]): boolean {
  return globs.some((glob) => globToRegExp(glob).test(candidatePath));
}

function collectWorkspaceExcludeGlobs(): string[] {
  const result: string[] = [];
  for (const key of ["files.exclude", "search.exclude"]) {
    const raw = configuration.get(key);
    if (!raw || typeof raw !== "object") {
      continue;
    }
    for (const [pattern, enabled] of Object.entries(raw as Record<string, unknown>)) {
      if (enabled === true) {
        result.push(pattern);
      }
    }
  }
  return result;
}

function isInWorkspace(uri: Uri): boolean {
  const normalized = normalizePath(uri.fsPath);
  return workspace.workspaceFolders.some((folder) => {
    const root = normalizePath(folder.uri.fsPath);
    return normalized === root || normalized.startsWith(`${root}/`);
  });
}

function notifyWatchers(kind: "create" | "delete", uri: Uri): void {
  const normalized = normalizePath(uri.fsPath);
  for (const watcher of fileWatchers) {
    if (!globToRegExp(watcher.glob).test(normalized)) {
      continue;
    }
    const listeners = kind === "create" ? watcher.createListeners : watcher.deleteListeners;
    for (const listener of listeners) {
      listener(uri);
    }
  }
}

export class EventEmitter<T = void> {
  private readonly listeners = new Set<(value: T) => void>();

  readonly event = (listener: (value: T) => void): Disposable => {
    this.listeners.add(listener);
    return new Disposable(() => {
      this.listeners.delete(listener);
    });
  };

  fire(value: T): void {
    for (const listener of this.listeners) {
      listener(value);
    }
  }

  dispose(): void {
    this.listeners.clear();
  }
}

export interface CancellationToken {
  isCancellationRequested: boolean;
  onCancellationRequested(listener: () => void): Disposable;
}

export class CancellationTokenSource {
  private cancelled = false;
  private readonly listeners = new Set<() => void>();

  get token(): CancellationToken {
    return {
      isCancellationRequested: this.cancelled,
      onCancellationRequested: (listener: () => void): Disposable => {
        if (this.cancelled) {
          listener();
          return new Disposable();
        }
        this.listeners.add(listener);
        return new Disposable(() => {
          this.listeners.delete(listener);
        });
      },
    };
  }

  cancel(): void {
    if (this.cancelled) {
      return;
    }
    this.cancelled = true;
    for (const listener of this.listeners) {
      listener();
    }
    this.listeners.clear();
  }

  dispose(): void {
    this.cancel();
  }
}

export class FileSystemError extends Error {}

export class Position {
  constructor(
    public readonly line: number,
    public readonly character: number,
  ) {}
}

export class Range {
  public readonly start: Position;
  public readonly end: Position;

  constructor(
    startLine: number | Position,
    startCharacter: number | Position,
    endLine?: number,
    endCharacter?: number,
  ) {
    if (startLine instanceof Position && startCharacter instanceof Position) {
      this.start = startLine;
      this.end = startCharacter;
      return;
    }

    this.start = new Position(startLine as number, startCharacter as number);
    this.end = new Position(endLine ?? 0, endCharacter ?? 0);
  }
}

export class Selection extends Range {}

export class ThemeIcon {
  constructor(public readonly id: string) {}
}

export class CodeLens {
  constructor(
    public readonly range: Range,
    public readonly command?: { command: string; title: string },
  ) {}
}

export class Uri {
  constructor(
    public readonly scheme: string,
    public readonly authority: string,
    public readonly path: string,
    public readonly query: string = "",
  ) {}

  static file(filePath: string): Uri {
    return new Uri("file", "", filePath, "");
  }

  static from(parts: {
    authority?: string;
    path?: string;
    query?: string;
    scheme: string;
  }): Uri {
    return new Uri(
      parts.scheme,
      (parts.authority ?? "").toLowerCase(),
      parts.path ?? "",
      parts.query ?? "",
    );
  }

  static parse(value: string): Uri {
    const match = value.match(/^([^:]+):(?:\/\/([^/]*))?([^?]*)(?:\?(.*))?$/);
    if (!match) {
      return new Uri("file", "", value, "");
    }
    return new Uri(match[1], (match[2] ?? "").toLowerCase(), match[3] ?? "", match[4] ?? "");
  }

  static joinPath(base: Uri, ...pathSegments: string[]): Uri {
    const joined = [
      base.path.replace(/\/+$/u, ""),
      ...pathSegments.map((segment) => segment.replace(/^\/+|\/+$/gu, "")),
    ]
      .filter((segment) => segment.length > 0)
      .join("/");
    return new Uri(base.scheme, base.authority, joined.startsWith("/") ? joined : `/${joined}`, base.query);
  }

  get fsPath(): string {
    return this.path;
  }

  toString(): string {
    const authority = this.authority ? `//${this.authority}` : "";
    const query = this.query ? `?${this.query}` : "";
    return `${this.scheme}:${authority}${this.path}${query}`;
  }
}

export const FileType = {
  File: 1,
  Directory: 2,
} as const;

export const ConfigurationTarget = {
  Global: 1,
  Workspace: 2,
  WorkspaceFolder: 3,
} as const;

export const TextEditorRevealType = {
  Default: 0,
  InCenter: 1,
  InCenterIfOutsideViewport: 2,
  AtTop: 3,
} as const;

export class WorkspaceEdit {
  readonly entries: Array<
    | { type: "replace"; uri: Uri; range: Range; text: string }
    | { type: "insert"; uri: Uri; position: Position; text: string }
    | { type: "createFile"; uri: Uri }
  > = [];

  replace(uri: Uri, range: Range, text: string): void {
    this.entries.push({ type: "replace", uri, range, text });
  }

  insert(uri: Uri, position: Position, text: string): void {
    this.entries.push({ type: "insert", uri, position, text });
  }

  createFile(uri: Uri, _options?: { ignoreIfExists?: boolean; overwrite?: boolean }): void {
    this.entries.push({ type: "createFile", uri });
  }
}

export interface QuickPickItem {
  description?: string;
  label: string;
}

export interface InputBoxOptions {
  ignoreFocusOut?: boolean;
  prompt?: string;
  title?: string;
}

class TextDocument {
  constructor(
    public readonly uri: Uri,
    private text: string,
  ) {}

  getText(): string {
    return this.text;
  }

  setText(text: string): void {
    this.text = text;
    files.set(this.uri.toString(), { text, type: FileType.File });
  }

  get lineCount(): number {
    return this.text.split("\n").length;
  }

  lineAt(line: number): { text: string } {
    return { text: this.text.split("\n")[line] ?? "" };
  }

  get isDirty(): boolean {
    return false;
  }

  async save(): Promise<boolean> {
    files.set(this.uri.toString(), { text: this.text, type: FileType.File });
    return true;
  }
}

export class ChatRequestTurn {
  constructor(public readonly prompt: string) {}
}

export class ChatResponseTurn {
  constructor(
    public readonly participant: string,
    public readonly result: { metadata?: Record<string, unknown> },
  ) {}
}

export const commands = {
  async executeCommand<T>(command: string, ...args: any[]): Promise<T> {
    if (command === "vscode.diff") {
      lastDiffCommand = {
        modified: args[1] as Uri,
        options: args[3],
        original: args[0] as Uri,
        title: args[2] as string,
      };
      return undefined as T;
    }

    const handler = commandHandlers.get(command);
    if (!handler) {
      return undefined as T;
    }

    return handler(...args) as T;
  },
  registerCommand(command: string, handler: CommandHandler): Disposable {
    commandHandlers.set(command, handler);
    return new Disposable(() => {
      commandHandlers.delete(command);
    });
  },
};

export const workspace = {
  asRelativePath(resource: string | Uri): string {
    const raw = typeof resource === "string" ? resource : resource.fsPath;
    const normalized = raw.replace(/\\/g, "/");
    const workspaceRoot = "/workspace";
    return normalized.startsWith(`${workspaceRoot}/`)
      ? normalized.slice(workspaceRoot.length + 1)
      : raw;
  },
  fs: {
    async readFile(uri: Uri): Promise<Uint8Array> {
      const entry = files.get(uri.toString());
      if (!entry) {
        throw new FileSystemError(`File not found: ${uri.toString()}`);
      }
      return Buffer.from(entry.text, "utf8");
    },
    async stat(uri: Uri): Promise<{ size: number; type: number }> {
      const entry = files.get(uri.toString());
      if (!entry) {
        throw new FileSystemError(`File not found: ${uri.toString()}`);
      }
      return { size: entry.text.length, type: entry.type };
    },
  },
  async applyEdit(edit: WorkspaceEdit): Promise<boolean> {
    for (const entry of edit.entries) {
      if (entry.type === "createFile") {
        files.set(entry.uri.toString(), { text: "", type: FileType.File });
        continue;
      }

      if (entry.type === "insert") {
        const current = files.get(entry.uri.toString())?.text ?? "";
        files.set(entry.uri.toString(), {
          text: `${entry.text}${current}`,
          type: FileType.File,
        });
        continue;
      }

      files.set(entry.uri.toString(), { text: entry.text, type: FileType.File });
    }
    return true;
  },
  getConfiguration(section: string) {
    return {
      get<T>(key: string, defaultValue?: T): T {
        const value = configuration.get(`${section}.${key}`);
        return (value as T | undefined) ?? (defaultValue as T);
      },
      inspect<T>(key: string): {
        defaultValue?: T;
        globalValue?: T;
        workspaceFolderValue?: T;
        workspaceValue?: T;
      } {
        const value = configuration.get(`${section}.${key}`) as T | undefined;
        return {
          globalValue: value,
        };
      },
      async update<T>(key: string, value: T): Promise<void> {
        const configKey = `${section}.${key}`;
        if (value === undefined) {
          configuration.delete(configKey);
        } else {
          configuration.set(configKey, value);
        }
        for (const listener of configurationListeners) {
          listener({
            affectsConfiguration(changedSection: string): boolean {
              return configKey === changedSection || configKey.startsWith(`${changedSection}.`);
            },
          });
        }
      },
    };
  },
  async findFiles(
    include: string,
    exclude?: string,
    maxResults?: number,
    token?: CancellationToken,
  ): Promise<Uri[]> {
    if (token?.isCancellationRequested) {
      return [];
    }
    const includeMatcher = globToRegExp(include);
    const excludeGlobs = exclude ? [exclude] : collectWorkspaceExcludeGlobs();
    const results: Uri[] = [];
    for (const [rawUri, entry] of files) {
      if (entry.type !== FileType.File) {
        continue;
      }
      const uri = Uri.parse(rawUri);
      if (!isInWorkspace(uri)) {
        continue;
      }
      const relativePath = normalizePath(workspace.asRelativePath(uri));
      if (!includeMatcher.test(relativePath)) {
        continue;
      }
      if (matchesAnyGlob(relativePath, excludeGlobs)) {
        continue;
      }
      results.push(uri);
      if (typeof maxResults === "number" && results.length >= maxResults) {
        break;
      }
    }
    return results;
  },
  createFileSystemWatcher(glob: string): {
    dispose(): void;
    onDidCreate(listener: (uri: Uri) => void): Disposable;
    onDidDelete(listener: (uri: Uri) => void): Disposable;
  } {
    const watcher = {
      createListeners: new Set<(uri: Uri) => void>(),
      deleteListeners: new Set<(uri: Uri) => void>(),
      glob,
    };
    fileWatchers.add(watcher);
    return {
      dispose() {
        fileWatchers.delete(watcher);
        watcher.createListeners.clear();
        watcher.deleteListeners.clear();
      },
      onDidCreate(listener: (uri: Uri) => void): Disposable {
        watcher.createListeners.add(listener);
        return new Disposable(() => {
          watcher.createListeners.delete(listener);
        });
      },
      onDidDelete(listener: (uri: Uri) => void): Disposable {
        watcher.deleteListeners.add(listener);
        return new Disposable(() => {
          watcher.deleteListeners.delete(listener);
        });
      },
    };
  },
  onDidChangeConfiguration(
    listener: (event: { affectsConfiguration(section: string): boolean }) => void,
  ): Disposable {
    configurationListeners.add(listener);
    return new Disposable(() => {
      configurationListeners.delete(listener);
    });
  },
  onDidChangeTextDocument(
    listener: (event: { document: TextDocument }) => void,
  ): Disposable {
    textDocumentChangeListeners.add(listener);
    return new Disposable(() => {
      textDocumentChangeListeners.delete(listener);
    });
  },
  openTextDocument: async (uri: Uri): Promise<TextDocument> => {
    const existing = textDocuments.find((document) => document.uri.toString() === uri.toString());
    if (existing) {
      return existing;
    }
    const provider = contentProviders.get(uri.scheme);
    if (provider) {
      const document = new TextDocument(uri, provider.provideTextDocumentContent(uri));
      textDocuments.push(document);
      return document;
    }

    const text = files.get(uri.toString())?.text ?? "";
    const document = new TextDocument(uri, text);
    textDocuments.push(document);
    return document;
  },
  registerTextDocumentContentProvider(scheme: string, provider: Provider): Disposable {
    contentProviders.set(scheme, provider);
    return new Disposable(() => {
      contentProviders.delete(scheme);
    });
  },
  textDocuments,
  workspaceFolders: [{ uri: Uri.file("/workspace") }],
};

export const window = {
  activeTextEditor: undefined as
    | {
        document: TextDocument;
        revealRange(range: Range, revealType?: number): void;
        selection: Selection;
      }
    | undefined,
  visibleTextEditors: [] as Array<{
    document: TextDocument;
    revealRange(range: Range, revealType?: number): void;
    selection: Selection;
  }>,
  async showInformationMessage(message: string, ...items: string[]): Promise<string | undefined> {
    return infoMessageHandler?.(message, items);
  },
  async showWarningMessage(message: string, ...items: string[]): Promise<string | undefined> {
    return warningMessageHandler?.(message, items);
  },
  async showOpenDialog(options: unknown): Promise<Uri[] | undefined> {
    return openDialogHandler?.(options);
  },
  async showQuickPick<T extends QuickPickItem>(
    items: readonly T[],
    _options?: unknown,
  ): Promise<T | undefined> {
    return quickPickHandler?.(items as QuickPickItem[]) as T | undefined;
  },
  async showInputBox(options: InputBoxOptions): Promise<string | undefined> {
    return inputBoxHandler?.(options);
  },
  async showTextDocument(
    document: TextDocument,
    _options?: unknown,
  ): Promise<{
    document: TextDocument;
    revealRange(range: Range, revealType?: number): void;
    selection: Selection;
  }> {
    const editor = {
      document,
      revealRange(range: Range, revealType?: number) {
        lastRevealRange = { range, revealType };
      },
      selection: new Selection(new Position(0, 0), new Position(0, 0)),
    };
    window.activeTextEditor = editor;
    window.visibleTextEditors = [editor];
    if (!textDocuments.some((entry) => entry.uri.toString() === document.uri.toString())) {
      textDocuments.push(document);
    }
    return { document };
  },
  createOutputChannel(_name: string): { appendLine(line: string): void; dispose(): void } {
    return {
      appendLine: () => undefined,
      dispose: () => undefined,
    };
  },
  tabGroups: {
    all: [],
    async close(): Promise<void> {},
  },
};

export const chat = {
  createChatParticipant(id: string, requestHandler: unknown) {
    return {
      dispose() {},
      id,
      iconPath: undefined,
      onDidReceiveFeedback: () => new Disposable(),
      requestHandler,
    };
  },
};

export const __testing = {
  deleteFile(filePath: string): void {
    const uri = Uri.file(filePath);
    files.delete(uri.toString());
    notifyWatchers("delete", uri);
  },
  get lastDiffCommand() {
    return lastDiffCommand;
  },
  get lastRevealRange() {
    return lastRevealRange;
  },
  readFile(filePath: string): string | undefined {
    return files.get(Uri.file(filePath).toString())?.text;
  },
  registerFile(filePath: string, text: string): void {
    const uri = Uri.file(filePath);
    files.set(uri.toString(), { text, type: FileType.File });
    notifyWatchers("create", uri);
  },
  registerDirectory(dirPath: string): void {
    const uri = Uri.file(dirPath);
    files.set(uri.toString(), { text: "", type: FileType.Directory });
    notifyWatchers("create", uri);
  },
  reset(): void {
    commandHandlers.clear();
    contentProviders.clear();
    files.clear();
    fileWatchers.clear();
    configuration.clear();
    configurationListeners.clear();
    textDocumentChangeListeners.clear();
    quickPickHandler = undefined;
    inputBoxHandler = undefined;
    infoMessageHandler = undefined;
    warningMessageHandler = undefined;
    openDialogHandler = undefined;
    lastDiffCommand = undefined;
    lastRevealRange = undefined;
    textDocuments.length = 0;
    window.activeTextEditor = undefined;
    window.visibleTextEditors = [];
    workspace.workspaceFolders = [{ uri: Uri.file("/workspace") }];
  },
  setConfiguration(key: string, value: unknown): void {
    configuration.set(key, value);
    for (const listener of configurationListeners) {
      listener({
        affectsConfiguration(section: string): boolean {
          return key === section || key.startsWith(`${section}.`);
        },
      });
    }
  },
  fireDidChangeTextDocument(document: TextDocument): void {
    for (const listener of textDocumentChangeListeners) {
      listener({ document });
    }
  },
  setInfoMessageHandler(handler: typeof infoMessageHandler): void {
    infoMessageHandler = handler;
  },
  setInputBoxHandler(handler: typeof inputBoxHandler): void {
    inputBoxHandler = handler;
  },
  setQuickPickHandler(handler: typeof quickPickHandler): void {
    quickPickHandler = handler;
  },
  setOpenDialogHandler(handler: typeof openDialogHandler): void {
    openDialogHandler = handler;
  },
  setWarningMessageHandler(handler: typeof warningMessageHandler): void {
    warningMessageHandler = handler;
  },
};
