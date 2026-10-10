import { afterEach, describe, expect, it, vi } from "vitest";
import * as vscode from "vscode";

import { TOMCAT_ADD_SELECTION_TO_CHAT_COMMAND, TEST_INFO_ACTION_ENV, TEST_WARNING_ACTION_ENV, TEST_SUPPRESS_EXIT_PROMPT_ENV } from "../src/constants";
import { nativeConfirmationHandler, showPromptMessage, TomcatSelectionCodeLensProvider } from "../src/extension";
import { setLocale } from "../src/shared/i18n";
import { en } from "../src/shared/i18n/en";

const nativeUi = (vscode as typeof vscode & { __testing: typeof import("./stubs/vscode").__testing }).__testing;

function offsetAt(text: string, position: vscode.Position): number {
  const lines = text.split("\n");
  let offset = 0;
  for (let line = 0; line < position.line; line += 1) {
    offset += (lines[line]?.length ?? 0) + 1;
  }
  return offset + position.character;
}

function positionAt(text: string, targetOffset: number): vscode.Position {
  const lines = text.split("\n");
  let remaining = Math.max(0, targetOffset);
  for (let line = 0; line < lines.length; line += 1) {
    const lineLength = lines[line]?.length ?? 0;
    if (remaining <= lineLength) {
      return new vscode.Position(line, remaining);
    }
    remaining -= lineLength + 1;
  }
  const lastLine = Math.max(0, lines.length - 1);
  return new vscode.Position(lastLine, lines[lastLine]?.length ?? 0);
}

function createEditor(
  filePath: string,
  text: string,
  start: vscode.Position,
  end: vscode.Position,
): vscode.TextEditor {
  const document = {
    getText(range?: { start: vscode.Position; end: vscode.Position }) {
      if (!range) {
        return text;
      }
      return text.slice(offsetAt(text, range.start), offsetAt(text, range.end));
    },
    offsetAt(position: vscode.Position) {
      return offsetAt(text, position);
    },
    positionAt(offset: number) {
      return positionAt(text, offset);
    },
    uri: vscode.Uri.file(filePath),
  } as unknown as vscode.TextDocument;

  return {
    document,
    selection: new vscode.Selection(start, end),
  } as unknown as vscode.TextEditor;
}

afterEach(() => {
  (vscode.window as typeof vscode.window & { activeTextEditor?: vscode.TextEditor }).activeTextEditor = undefined;
  setLocale("en");
  nativeUi.setWarningMessageHandler(undefined);
  nativeUi.setInfoMessageHandler(undefined);
  vi.unstubAllEnvs();
});

describe("TomcatSelectionCodeLensProvider", () => {
  it("refreshes the native lens on language change without changing command identity", () => {
    const provider = new TomcatSelectionCodeLensProvider();
    const editor = createEditor("/workspace/a.ts", "abc", new vscode.Position(0, 0), new vscode.Position(0, 2));
    (vscode.window as typeof vscode.window & { activeTextEditor?: vscode.TextEditor }).activeTextEditor = editor;
    const changed = vi.fn();
    const listener = provider.onDidChangeCodeLenses(changed);
    try {
      expect(provider.provideCodeLenses(editor.document)[0]?.command?.title).toBe(en["host.addSelection"]);
      setLocale("zh-CN");
      expect(changed).toHaveBeenCalledOnce();
      expect(provider.provideCodeLenses(editor.document)[0]?.command?.command).toBe(TOMCAT_ADD_SELECTION_TO_CHAT_COMMAND);
      provider.dispose();
      setLocale("en");
      expect(changed).toHaveBeenCalledOnce();
    } finally { listener.dispose(); provider.dispose(); setLocale("en"); }
  });

  it("shows an Add to Tomcat Chat lens for a non-empty selection", () => {
    const provider = new TomcatSelectionCodeLensProvider();
    const editor = createEditor(
      "/workspace/src/app.ts",
      "const alpha = 1;\nconst beta = 2;\n",
      new vscode.Position(1, 0),
      new vscode.Position(1, "const beta = 2;".length),
    );
    (vscode.window as typeof vscode.window & { activeTextEditor?: vscode.TextEditor }).activeTextEditor = editor;

    const lenses = provider.provideCodeLenses(editor.document);

    expect(lenses).toHaveLength(1);
    expect(lenses[0]?.command).toEqual({
      command: TOMCAT_ADD_SELECTION_TO_CHAT_COMMAND,
      title: "Add to Tomcat Chat",
    });
    expect(lenses[0]?.range.start.line).toBe(1);
    provider.dispose();
  });

  it("returns no lenses for an empty selection", () => {
    const provider = new TomcatSelectionCodeLensProvider();
    const editor = createEditor(
      "/workspace/src/app.ts",
      "const alpha = 1;\n",
      new vscode.Position(0, 0),
      new vscode.Position(0, 0),
    );
    (vscode.window as typeof vscode.window & { activeTextEditor?: vscode.TextEditor }).activeTextEditor = editor;

    expect(provider.provideCodeLenses(editor.document)).toEqual([]);
    provider.dispose();
  });
});

describe("native action identity", () => {
  it.each(["info", "warning"] as const)("returns an action ID despite identical %s titles", async severity => {
    vi.stubEnv(TEST_INFO_ACTION_ENV, "");
    vi.stubEnv(TEST_WARNING_ACTION_ENV, "");
    vi.stubEnv(TEST_SUPPRESS_EXIT_PROMPT_ENV, "0");
    const select = (_message: string, items: Array<string | vscode.MessageItem>) => {
      const first = items[0] as vscode.MessageItem;
      const second = items[1] as vscode.MessageItem;
      expect(first.title).toBe(en["host.action.retry"]);
      first.title = second.title = "same title";
      return second;
    };
    if (severity === "info") nativeUi.setInfoMessageHandler(select);
    else nativeUi.setWarningMessageHandler(select);
    expect(await showPromptMessage([], severity, "fixture", ["host.action.retry", "host.action.logs"]))
      .toBe("host.action.logs");
  });

  it("auto-selects only stable IDs, not displayed labels", async () => {
    vi.stubEnv(TEST_SUPPRESS_EXIT_PROMPT_ENV, "1");
    vi.stubEnv(TEST_WARNING_ACTION_ENV, en["host.action.retry"]);
    expect(await showPromptMessage([], "warning", "fixture", ["host.action.retry"])).toBeUndefined();
    vi.stubEnv(TEST_WARNING_ACTION_ENV, "host.action.retry");
    setLocale("zh-CN");
    expect(await showPromptMessage([], "warning", "fixture", ["host.action.retry"])).toBe("host.action.retry");
  });

  it.each(["en", "zh-CN"] as const)("keeps native permission choices distinct in %s", async locale => {
    setLocale(locale);
    nativeUi.setWarningMessageHandler((_message, items) => {
      const once = items[0] as vscode.MessageItem;
      const folder = items[1] as vscode.MessageItem;
      if (locale === "en") {
        expect(once.title).toBe(en["host.allowOnce"]);
        expect(folder.title).toBe(en["host.allowFolder"]);
      }
      once.title = folder.title = "same title";
      return folder;
    });
    const result = await nativeConfirmationHandler({
      type: "control_request", requestId: "permission", sessionId: "s", subtype: "confirmation",
      payload: { preview: "fixture preview", suggestedRoot: "/workspace" },
    }, { generation: 1, signal: new AbortController().signal });
    expect(result).toEqual({ kind: "response", sessionId: "s", payload: { decision: "allow_and_persist_root", root: "/workspace" } });
  });

  it.each(["closed", "cloned-title", "aborted"] as const)("does not authorize a %s native answer", async outcome => {
    const controller = new AbortController();
    nativeUi.setWarningMessageHandler((_message, items) => {
      if (outcome === "closed") return undefined;
      if (outcome === "cloned-title") return { ...(items[0] as vscode.MessageItem) };
      controller.abort();
      return items[0];
    });
    const result = await nativeConfirmationHandler({
      type: "control_request", requestId: "permission", sessionId: "s", subtype: "confirmation",
      payload: { preview: "fixture preview" },
    }, { generation: 1, signal: controller.signal });
    expect(result).toEqual(outcome === "aborted"
      ? { kind: "cancel", payload: { reason: "host_disconnected" } }
      : { kind: "response", sessionId: "s", payload: { decision: "deny" } });
  });
});
