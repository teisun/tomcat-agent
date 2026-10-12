import type { Editor } from "@tiptap/core";
import { PluginKey } from "@tiptap/pm/state";
import Suggestion, { exitSuggestion } from "@tiptap/suggestion";
import type { InstructionCard, SharedSlashCommand } from "../../../src/serveClient/wire";
import { withOccurrence } from "../../../src/shared/composerOccurrences";
import { buildSlashMenuSections, type SlashMenuItem } from "../slashMenu";
import { isTriggerBoundary } from "./suggestionBoundary";

const SLASH_KEY = new PluginKey("tomcat-resource-slash-command");
export interface SlashSuggestionState { query: string; leading: boolean }
interface Options {
  getCommands(): readonly SharedSlashCommand[];
  getCatalog(): readonly InstructionCard[];
  getKeyHandler(): ((event: KeyboardEvent) => boolean) | undefined;
  isComposing(): boolean;
  onState(state: SlashSuggestionState | null): void;
}

export function createSlashCommandSuggestion(options: Options) {
  let activeEditor: Editor | null = null;
  let select: ((command: SlashMenuItem) => void) | null = null;
  const close = () => { if (activeEditor && !activeEditor.isDestroyed) exitSuggestion(activeEditor.view, SLASH_KEY); };
  return {
    close,
    command(command: SlashMenuItem): boolean {
      if (!select) return false;
      select(command);
      return true;
    },
    attach(editor: Editor): () => void {
      editor.registerPlugin(Suggestion<SlashMenuItem, SlashMenuItem>({
        editor,pluginKey:SLASH_KEY,char:"/",allowSpaces:false,allowedPrefixes:[" ","\t","\n"],
        allow: ({ editor,state,range }) => editor.isEditable && !options.isComposing() && !editor.view.composing && isTriggerBoundary({state},range),
        items: ({query}) => buildSlashMenuSections(options.getCommands(),query,options.getCatalog()).flatMap((section) => section.items),
        command: ({editor,range,props}) => {
          if (props.kind === "terminal") editor.chain().focus().insertContentAt(range,`/${props.name} `).run();
          else editor.chain().focus().insertContentAt(range,[{type:"instruction",attrs:withOccurrence({type:"instruction",kind:props.kind,resourceId:props.id,label:`/${props.name}`,path:props.path})},{type:"text",text:" "}]).run();
        },
        render: () => ({
          onStart: (props) => {
            activeEditor = props.editor;select = props.command;
            options.onState({query:props.query,leading:props.editor.state.doc.textBetween(0,props.range.from,"\n","\0").trim().length === 0});
          },
          onUpdate: (props) => {
            select = props.command;
            options.onState({query:props.query,leading:props.editor.state.doc.textBetween(0,props.range.from,"\n","\0").trim().length === 0});
          },
          onExit: () => { activeEditor = null;select = null;options.onState(null); },
          onKeyDown: ({event}) => !options.isComposing() && !event.isComposing && (options.getKeyHandler()?.(event) ?? false),
        }),
      }),(plugin,plugins) => [plugin,...plugins]);
      return () => { close();editor.unregisterPlugin(SLASH_KEY);activeEditor = null;select = null;options.onState(null); };
    },
  };
}
