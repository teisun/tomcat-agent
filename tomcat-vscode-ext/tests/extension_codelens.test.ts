import { afterEach, describe, expect, it } from "vitest";
import * as vscode from "vscode";

import { TOMCAT_ADD_SELECTION_TO_CHAT_COMMAND } from "../src/constants";
import { TomcatSelectionCodeLensProvider } from "../src/extension";

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
});

describe("TomcatSelectionCodeLensProvider", () => {
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
