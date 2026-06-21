type CommandHandler = (...args: any[]) => any;
type Provider = { provideTextDocumentContent(uri: Uri): string };
type FileEntry = { text: string };

const commandHandlers = new Map<string, CommandHandler>();
const contentProviders = new Map<string, Provider>();
const files = new Map<string, FileEntry>();
const configuration = new Map<string, unknown>();
const configurationListeners = new Set<
  (event: { affectsConfiguration(section: string): boolean }) => void
>();

let quickPickHandler: ((items: QuickPickItem[]) => any) | undefined;
let inputBoxHandler: ((options: InputBoxOptions) => any) | undefined;
let infoMessageHandler: ((message: string, items: string[]) => any) | undefined;
let warningMessageHandler: ((message: string, items: string[]) => any) | undefined;
let lastDiffCommand:
  | {
      original: Uri;
      modified: Uri;
      options?: unknown;
      title?: string;
    }
  | undefined;

export class Disposable {
  constructor(private readonly callback: () => void = () => undefined) {}

  dispose(): void {
    this.callback();
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
      parts.authority ?? "",
      parts.path ?? "",
      parts.query ?? "",
    );
  }

  static parse(value: string): Uri {
    const match = value.match(/^([^:]+):(?:\/\/([^/]*))?([^?]*)(?:\?(.*))?$/);
    if (!match) {
      return new Uri("file", "", value, "");
    }
    return new Uri(match[1], match[2] ?? "", match[3] ?? "", match[4] ?? "");
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
    files.set(this.uri.toString(), { text });
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
    files.set(this.uri.toString(), { text: this.text });
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
  fs: {
    async readFile(uri: Uri): Promise<Uint8Array> {
      const entry = files.get(uri.toString());
      if (!entry) {
        throw new FileSystemError(`File not found: ${uri.toString()}`);
      }
      return Buffer.from(entry.text, "utf8");
    },
    async stat(uri: Uri): Promise<{ size: number }> {
      const entry = files.get(uri.toString());
      if (!entry) {
        throw new FileSystemError(`File not found: ${uri.toString()}`);
      }
      return { size: entry.text.length };
    },
  },
  async applyEdit(edit: WorkspaceEdit): Promise<boolean> {
    for (const entry of edit.entries) {
      if (entry.type === "createFile") {
        files.set(entry.uri.toString(), { text: "" });
        continue;
      }

      if (entry.type === "insert") {
        const current = files.get(entry.uri.toString())?.text ?? "";
        files.set(entry.uri.toString(), { text: `${entry.text}${current}` });
        continue;
      }

      files.set(entry.uri.toString(), { text: entry.text });
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
  openTextDocument: async (uri: Uri): Promise<TextDocument> => {
    const provider = contentProviders.get(uri.scheme);
    if (provider) {
      return new TextDocument(uri, provider.provideTextDocumentContent(uri));
    }

    const text = files.get(uri.toString())?.text ?? "";
    return new TextDocument(uri, text);
  },
  registerTextDocumentContentProvider(scheme: string, provider: Provider): Disposable {
    contentProviders.set(scheme, provider);
    return new Disposable(() => {
      contentProviders.delete(scheme);
    });
  },
  workspaceFolders: [{ uri: Uri.file("/workspace") }],
};

export const window = {
  async showInformationMessage(message: string, ...items: string[]): Promise<string | undefined> {
    return infoMessageHandler?.(message, items);
  },
  async showWarningMessage(message: string, ...items: string[]): Promise<string | undefined> {
    return warningMessageHandler?.(message, items);
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
  async showTextDocument(document: TextDocument, _options?: unknown): Promise<{ document: TextDocument }> {
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
    files.delete(Uri.file(filePath).toString());
  },
  get lastDiffCommand() {
    return lastDiffCommand;
  },
  readFile(filePath: string): string | undefined {
    return files.get(Uri.file(filePath).toString())?.text;
  },
  registerFile(filePath: string, text: string): void {
    files.set(Uri.file(filePath).toString(), { text });
  },
  reset(): void {
    commandHandlers.clear();
    contentProviders.clear();
    files.clear();
    configuration.clear();
    configurationListeners.clear();
    quickPickHandler = undefined;
    inputBoxHandler = undefined;
    infoMessageHandler = undefined;
    warningMessageHandler = undefined;
    lastDiffCommand = undefined;
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
  setInfoMessageHandler(handler: typeof infoMessageHandler): void {
    infoMessageHandler = handler;
  },
  setInputBoxHandler(handler: typeof inputBoxHandler): void {
    inputBoxHandler = handler;
  },
  setQuickPickHandler(handler: typeof quickPickHandler): void {
    quickPickHandler = handler;
  },
  setWarningMessageHandler(handler: typeof warningMessageHandler): void {
    warningMessageHandler = handler;
  },
};
