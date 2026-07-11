import { Extension, type Editor, type Range } from "@tiptap/core";
import Suggestion, {
  exitSuggestion,
  type SuggestionKeyDownProps,
} from "@tiptap/suggestion";
import { PluginKey } from "@tiptap/pm/state";

import type { ContextSearchMatch, WebviewReference } from "../types";

const MENTION_PLUGIN_KEY = new PluginKey("tomcat-context-search-mention");

export interface MentionSuggestionController {
  close(): void;
  command(match: ContextSearchMatch): boolean;
  extension: Extension;
}

interface CreateMentionSuggestionOptions {
  editorHasReference(editor: Editor, reference: WebviewReference): boolean;
  getKeyHandler(): ((event: KeyboardEvent) => boolean) | null | undefined;
  isComposing(): boolean;
  onClose(): void;
  onOpen(): void;
  onQueryChange(query: string): void;
  referenceNodeName: string;
}

function isWhitespaceBoundary(editor: Editor, range: Range): boolean {
  if (range.from <= 1) {
    return true;
  }
  const previousCharacter = editor.state.doc.textBetween(
    range.from - 1,
    range.from,
    "\n",
    "\0",
  );
  return previousCharacter === "" || /\s/u.test(previousCharacter);
}

function insertReferenceAtRange(
  editor: Editor,
  range: Range,
  match: ContextSearchMatch,
  options: Pick<CreateMentionSuggestionOptions, "editorHasReference" | "referenceNodeName">,
): void {
  const chain = editor.chain().focus().deleteRange(range);
  if (options.editorHasReference(editor, match.reference)) {
    chain.run();
    return;
  }
  chain
    .insertContent([
      {
        attrs: match.reference,
        type: options.referenceNodeName,
      },
      {
        text: " ",
        type: "text",
      },
    ])
    .run();
}

export function createMentionSuggestion(
  options: CreateMentionSuggestionOptions,
): MentionSuggestionController {
  let activeEditor: Editor | null = null;
  let currentCommand: ((match: ContextSearchMatch) => void) | null = null;

  return {
    close(): void {
      if (!activeEditor) {
        return;
      }
      exitSuggestion(activeEditor.view, MENTION_PLUGIN_KEY);
    },
    command(match: ContextSearchMatch): boolean {
      if (!currentCommand) {
        return false;
      }
      currentCommand(match);
      return true;
    },
    extension: Extension.create({
      name: "tomcatContextSearchMentionSuggestion",
      addProseMirrorPlugins() {
        return [
          Suggestion<never, ContextSearchMatch>({
            allow: ({ editor, range }) => {
              if (options.isComposing() || editor.view.composing) {
                return false;
              }
              return isWhitespaceBoundary(editor, range);
            },
            allowSpaces: false,
            allowedPrefixes: [" ", "\t", "\n"],
            char: "@",
            command: ({ editor, range, props }) => {
              insertReferenceAtRange(editor, range, props, options);
            },
            editor: this.editor,
            items: () => [],
            pluginKey: MENTION_PLUGIN_KEY,
            render: () => ({
              onExit: () => {
                activeEditor = null;
                currentCommand = null;
                options.onClose();
              },
              onKeyDown: ({ event }: SuggestionKeyDownProps) =>
                options.getKeyHandler()?.(event) ?? false,
              onStart: (props) => {
                activeEditor = props.editor;
                currentCommand = props.command;
                options.onOpen();
                options.onQueryChange(props.query);
              },
              onUpdate: (props) => {
                activeEditor = props.editor;
                currentCommand = props.command;
                options.onQueryChange(props.query);
              },
            }),
          }),
        ];
      },
    }),
  };
}
