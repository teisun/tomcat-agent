import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
  type DragEvent,
  type ReactNode,
} from "react";

import { Node as TiptapNode, type JSONContent } from "@tiptap/core";
import Placeholder from "@tiptap/extension-placeholder";
import StarterKit from "@tiptap/starter-kit";
import {
  EditorContent,
  NodeViewWrapper,
  ReactNodeViewRenderer,
  useEditor,
  type NodeViewProps,
} from "@tiptap/react";

import { prepareAttachment, type PreparedAttachment } from "../attachments/imagePipeline";
import { referenceIdentity } from "../contextReferences";
import { isSupportedAttachmentMime } from "../../../src/shared/attachmentProtocol";
import type {
  ContextSearchMatch,
  WebviewPlanFileState,
  WebviewMessageSegment,
  WebviewReference,
} from "../types";
import {
  ContextSearchDropdown,
  type ContextSearchDropdownHandle,
} from "./ContextSearchDropdown";
import type { Speed } from "../../../src/shared/modelSpeed";
import { buildPickerModels } from "./buildPickerModels";
import { createMentionSuggestion } from "./mentionSuggestion";
import type { SharedSlashCommand } from "../../../src/serveClient/wire";
import { buildSlashMenuSections } from "../slashMenu";
import { SlashCommandMenu, type SlashCommandMenuHandle } from "./SlashCommandMenu";
import { createSlashCommandSuggestion, type SlashSuggestionState } from "./slashCommandSuggestion";
import { ReferenceChip } from "./ReferenceChip";
import { InvocationNode, instructionFromAttrs } from "./InvocationNode";
import { withOccurrence } from "../../../src/shared/composerOccurrences";
import { ModelPicker, type ModelPickerModel } from "./ModelPicker";
import { useT } from "../i18n/LocaleProvider";
import type { MessageKey, Translator } from "../../../src/shared/i18n";

function formatPlanStatus(planState: WebviewPlanFileState | null | undefined, t: Translator): string | null {
  if (!planState) {
    return null;
  }
  return `${t("term.mode.plan")}: ${t(`term.planState.${planState}`)}`;
}

const REFERENCE_NODE_NAME = "reference";
const DROP_URI_SCHEMES = /^(file|vscode-file|vscode-remote):/i;
const IMAGE_ATTACHMENT_EXTENSIONS = new Set([".gif", ".jpeg", ".jpg", ".png", ".webp"]);
const TEST_SET_COMPOSER_VALUE_EVENT = "tomcat:test:set-composer-value";
const MODE_OPTIONS = [
  { labelKey: "term.mode.chat", value: "chat" },
  { labelKey: "term.mode.plan", value: "plan" },
] as const;

function hasCapability(capabilities: string[], capability: "files" | "vision"): boolean {
  return capabilities.includes(capability);
}

function extractUriExtension(uri: string): string | null {
  try {
    const pathname = decodeURIComponent(new URL(uri).pathname).toLowerCase();
    const lastDot = pathname.lastIndexOf(".");
    if (lastDot < 0) {
      return null;
    }
    return pathname.slice(lastDot);
  } catch {
    return null;
  }
}

function buildPickerHint(capabilities?: string[]): MessageKey | null {
  if (!capabilities) {
    return null;
  }
  const supportsVision = hasCapability(capabilities, "vision");
  const supportsFiles = hasCapability(capabilities, "files");
  if (supportsVision && supportsFiles) {
    return null;
  }
  if (!supportsVision && !supportsFiles) {
    return "composer.hint.pickBoth";
  }
  if (!supportsVision) {
    return "composer.hint.pickImage";
  }
  return "composer.hint.pickPdf";
}

function buildDropHint(capabilities: string[] | undefined, uris: string[]): MessageKey | null {
  if (!capabilities) {
    return null;
  }
  const supportsVision = hasCapability(capabilities, "vision");
  const supportsFiles = hasCapability(capabilities, "files");
  if (supportsVision && supportsFiles) {
    return null;
  }
  const includesImage = uris.some((uri) => {
    const extension = extractUriExtension(uri);
    return extension ? IMAGE_ATTACHMENT_EXTENSIONS.has(extension) : false;
  });
  const includesPdf = uris.some((uri) => extractUriExtension(uri) === ".pdf");
  if (includesImage && !supportsVision && includesPdf && !supportsFiles) {
    return "composer.hint.dropBoth";
  }
  if (includesImage && !supportsVision) {
    return "composer.hint.dropImage";
  }
  if (includesPdf && !supportsFiles) {
    return "composer.hint.dropPdf";
  }
  return null;
}

function buildPasteHint(capabilities: string[] | undefined, files: File[]): MessageKey | null {
  if (!capabilities) {
    return null;
  }
  const supportsVision = hasCapability(capabilities, "vision");
  const supportsFiles = hasCapability(capabilities, "files");
  if (supportsVision && supportsFiles) {
    return null;
  }
  const includesImage = files.some((file) => file.type.startsWith("image/"));
  const includesPdf = files.some((file) => file.type === "application/pdf");
  if (includesImage && !supportsVision && includesPdf && !supportsFiles) {
    return "composer.hint.pasteBoth";
  }
  if (includesImage && !supportsVision) {
    return "composer.hint.pasteImage";
  }
  if (includesPdf && !supportsFiles) {
    return "composer.hint.pastePdf";
  }
  return null;
}

type FileWithSourcePath = File & {
  path?: string;
  sourcePath?: string;
};

function extractFileSourcePath(file: File): string | null {
  const withSourcePath = file as FileWithSourcePath;
  if (typeof withSourcePath.sourcePath === "string" && withSourcePath.sourcePath.trim()) {
    return withSourcePath.sourcePath;
  }
  if (typeof withSourcePath.path === "string" && withSourcePath.path.trim()) {
    return withSourcePath.path;
  }
  return null;
}

function modeLabel(value: "chat" | "plan", t: Translator): string {
  return t(value === "plan" ? "term.mode.plan" : "term.mode.chat");
}

export interface ComposerDraft {
  hasContent: boolean;
  segments: WebviewMessageSegment[];
  text: string;
}

export interface ComposerHandle {
  clear(): void;
  closeMention(): void;
  getDraft(): ComposerDraft;
  insertReference(reference: WebviewReference): void;
  insertReferences(references: readonly WebviewReference[]): void;
  replaceDraft(draft: ComposerDraft): void;
}

type ComposerNoticeTone = "info" | "active" | "warning" | "plan";

interface ComposerNotice {
  id: "capability" | "drag" | "plan";
  text: string;
  tone: ComposerNoticeTone;
}

function normalizeReferenceAttrs(attrs: Record<string, unknown>): WebviewReference | null {
  if (
    (attrs.kind !== "selection" && attrs.kind !== "file") ||
    typeof attrs.label !== "string" ||
    typeof attrs.path !== "string"
  ) {
    return null;
  }
  return {
    kind: attrs.kind,
    occurrenceId: typeof attrs.occurrenceId === "string" ? attrs.occurrenceId : undefined,
    label: attrs.label,
    lineEnd: typeof attrs.lineEnd === "number" ? attrs.lineEnd : null,
    lineStart: typeof attrs.lineStart === "number" ? attrs.lineStart : null,
    path: attrs.path,
    text: typeof attrs.text === "string" ? attrs.text : null,
    type: "reference",
  };
}

function pushTextSegment(segments: WebviewMessageSegment[], text: string): void {
  if (!text) {
    return;
  }
  const last = segments.at(-1);
  if (last?.type === "text") {
    last.text += text;
    return;
  }
  segments.push({
    text,
    type: "text",
  });
}

function appendProjectionText(chunks: string[], text: string): void {
  if (text) {
    chunks.push(text);
  }
}

function walkContentNode(
  node: JSONContent,
  segments: WebviewMessageSegment[],
  projection: string[],
): void {
  if (node.type === "text" && typeof node.text === "string") {
    pushTextSegment(segments, node.text);
    appendProjectionText(projection, node.text);
    return;
  }
  if (node.type === "hardBreak") {
    pushTextSegment(segments, "\n");
    appendProjectionText(projection, "\n");
    return;
  }
  if (node.type === "instruction" && node.attrs) {
    const instruction = instructionFromAttrs(node.attrs);
    if (instruction) { segments.push(instruction); appendProjectionText(projection, instruction.label); }
    return;
  }
  if (node.type === REFERENCE_NODE_NAME && node.attrs) {
    const reference = normalizeReferenceAttrs(node.attrs as Record<string, unknown>);
    if (reference) {
      segments.push(reference);
      appendProjectionText(projection, reference.label);
    }
    return;
  }
  for (const child of node.content ?? []) {
    walkContentNode(child, segments, projection);
  }
}

export function serializeComposerDocument(
  doc: JSONContent | null | undefined,
): ComposerDraft {
  const segments: WebviewMessageSegment[] = [];
  const projection: string[] = [];
  const blocks = doc?.content ?? [];
  blocks.forEach((block, index) => {
    if (index > 0) {
      pushTextSegment(segments, "\n\n");
      appendProjectionText(projection, "\n\n");
    }
    walkContentNode(block, segments, projection);
  });
  return {
    hasContent: segments.some(
      (segment) => segment.type !== "text" || segment.text.trim().length > 0,
    ),
    segments,
    text: projection.join(""),
  };
}

function pushTextNodes(content: JSONContent[], text: string): void {
  const parts = text.split("\n");
  parts.forEach((part, index) => {
    if (part.length > 0) {
      content.push({
        text: part,
        type: "text",
      });
    }
    if (index < parts.length - 1) {
      content.push({
        type: "hardBreak",
      });
    }
  });
}

function createComposerDocument(segments: WebviewMessageSegment[]): JSONContent {
  const paragraphContent: JSONContent[] = [];
  segments.forEach((segment) => {
    if (segment.type === "text") {
      pushTextNodes(paragraphContent, segment.text);
      return;
    }
    paragraphContent.push({
      attrs: withOccurrence(segment),
      type: segment.type === "instruction" ? "instruction" : REFERENCE_NODE_NAME,
    });
  });
  return {
    content: [{
      content: paragraphContent,
      type: "paragraph",
    }],
    type: "doc",
  };
}

function parseUriList(value: string): string[] {
  return value
    .split(/\r?\n/)
    .map((entry) => entry.trim())
    .filter((entry) => entry.length > 0 && !entry.startsWith("#"));
}

function parseJsonUriArray(value: string): string[] {
  try {
    const parsed = JSON.parse(value);
    return Array.isArray(parsed)
      ? parsed.filter((entry): entry is string => typeof entry === "string")
      : [];
  } catch {
    return [];
  }
}

function filePathToUriString(filePath: string): string {
  const normalized = filePath.replace(/\\/g, "/");
  return `file://${normalized.startsWith("/") ? "" : "/"}${encodeURI(normalized)}`;
}

export function extractDropUris(dataTransfer: DataTransfer): string[] {
  const candidates = [
    ...parseJsonUriArray(dataTransfer.getData("resourceurls")),
    ...parseJsonUriArray(dataTransfer.getData("ResourceURLs")),
    ...parseUriList(dataTransfer.getData("application/vnd.code.uri-list")),
    ...parseJsonUriArray(dataTransfer.getData("CodeFiles")),
    ...parseUriList(dataTransfer.getData("CodeFiles")),
    ...parseUriList(dataTransfer.getData("text/uri-list")),
    ...Array.from(dataTransfer.files)
      .map((file) => (file as File & { path?: string }).path)
      .filter((entry): entry is string => typeof entry === "string" && entry.length > 0)
      .map(filePathToUriString),
  ];
  const seen = new Set<string>();
  return candidates.filter((entry) => {
    if (!DROP_URI_SCHEMES.test(entry) || seen.has(entry)) {
      return false;
    }
    seen.add(entry);
    return true;
  });
}

function ReferenceNodeView({
  deleteNode,
  node,
}: NodeViewProps) {
  const reference = normalizeReferenceAttrs(node.attrs as Record<string, unknown>);
  if (!reference) {
    return null;
  }
  return (
    <NodeViewWrapper as="span" className="tc-reference-node" contentEditable={false}>
      <ReferenceChip onRemove={() => deleteNode()} reference={reference} testId="composer-reference-chip" />
    </NodeViewWrapper>
  );
}

const ReferenceNode = TiptapNode.create({
  name: REFERENCE_NODE_NAME,
  group: "inline",
  inline: true,
  atom: true,
  selectable: false,
  addAttributes() {
    return {
      occurrenceId: { default: null, parseHTML: () => crypto.randomUUID() },
      kind: {
        default: "file",
      },
      label: {
        default: "",
      },
      lineEnd: {
        default: null,
      },
      lineStart: {
        default: null,
      },
      path: {
        default: "",
      },
      text: {
        default: null,
      },
    };
  },
  parseHTML() {
    return [{ tag: "span[data-tomcat-reference]" }];
  },
  renderHTML({ HTMLAttributes }) {
    return ["span", { ...HTMLAttributes, "data-tomcat-reference": "true" }];
  },
  addNodeView() {
    return ReactNodeViewRenderer(ReferenceNodeView);
  },
});

function editorHasReference(editor: NonNullable<ReturnType<typeof useEditor>>, reference: WebviewReference): boolean {
  const target = referenceIdentity(reference);
  let found = false;
  editor.state.doc.descendants((node) => {
    if (node.type.name !== REFERENCE_NODE_NAME) {
      return true;
    }
    const existing = normalizeReferenceAttrs(node.attrs as Record<string, unknown>);
    if (existing && referenceIdentity(existing) === target) {
      found = true;
      return false;
    }
    return true;
  });
  return found;
}

const EMPTY_DRAFT: ComposerDraft = {
  hasContent: false,
  segments: [],
  text: "",
};

function sameDraft(left: ComposerDraft, right: ComposerDraft): boolean {
  return (
    left.hasContent === right.hasContent &&
    left.text === right.text &&
    JSON.stringify(left.segments) === JSON.stringify(right.segments)
  );
}

export interface ComposerProps {
  /** Optional presentation only; all compose locations keep the same surface. */
  header?: ReactNode;
  beforeEditor?: ReactNode;
  instanceId?: string;
  initialDraft?: ComposerDraft;
  onCancelEdit?(): void;
  allowBusyInput?: boolean;
  submitAriaLabel?: string;
  hideDragHint?: boolean;
  submitDisabled?: boolean;
  hasAttachments?: boolean;
  attachmentsPending?: boolean;
  availableModelDetails?: Record<string, ModelPickerModel>;
  availableModelReasoningLevels?: Record<string, string[]>;
  availableModels: string[];
  slashCommands?: readonly SharedSlashCommand[];
  instructionCatalog?: readonly import("../../../src/serveClient/wire").InstructionCard[];
  onSlashOpen?(): void;
  commandPending?: boolean;
  busy?: boolean;
  canInterrupt: boolean;
  canPrompt: boolean;
  /** Real session configuration permission, independent of an editor's submit/Stop state. */
  canChangeConfig?: boolean;
  contextSearchLoading: boolean;
  contextSearchMatches: ContextSearchMatch[];
  contextSearchQuery: string;
  contextSearchTruncated: boolean;
  contextWindowValue?: number | null;
  contextLabel: string;
  modelCapabilities?: string[] | undefined;
  modeValue: "chat" | "plan";
  modelValue: string;
  onContextSearchClose(): void;
  onContextSearchOpen(): void;
  onContextSearchQueryChange(query: string): void;
  supportedReasoningLevels?: string[] | undefined;
  thinkingLevelValue: string;
  onAttachFiles?(files: PreparedAttachment[]): void;
  /** Called synchronously when paste preparation starts so App can bind it to the source session. */
  onPrepareAttachments?(work: Promise<PreparedAttachment[]>): void;
  onPickContext(): void;
  onContextWindowChange(modelId: string, contextWindow: number): void;
  onDraftChange(draft: ComposerDraft): void;
  onModeChange(value: "chat" | "plan"): void;
  onModelChange(value: string): void;
  onOpenModelSettings?(): void;
  onResolveDrop(uris: string[]): void;
  onThinkingLevelChange(modelId: string, value: string): void;
  onSpeedChange(modelId: string, speed: Speed): void;
  onInterrupt?(): void;
  onSubmit(): void;
  planState?: WebviewPlanFileState | null;
}

export const Composer = forwardRef<ComposerHandle, ComposerProps>(function Composer({
  instanceId = "",
  header,
  beforeEditor,
  initialDraft,
  onCancelEdit,
  availableModelDetails,
  availableModelReasoningLevels,
  availableModels,
  busy = false,
  canChangeConfig: configAllowed = !busy,
  allowBusyInput = false,
  submitAriaLabel,
  hideDragHint = false,
  submitDisabled = false,
  hasAttachments = false,
  attachmentsPending = false,
  slashCommands = [],
  instructionCatalog = [],
  onSlashOpen,
  commandPending = false,
  canInterrupt,
  canPrompt,
  contextSearchLoading,
  contextSearchMatches,
  contextSearchQuery,
  contextSearchTruncated,
  contextWindowValue,
  contextLabel,
  modelCapabilities,
  modeValue,
  modelValue,
  onAttachFiles,
  onPrepareAttachments,
  onContextSearchClose,
  onContextSearchOpen,
  onContextSearchQueryChange,
  supportedReasoningLevels,
  onContextWindowChange,
  thinkingLevelValue,
  onPickContext,
  onDraftChange,
  onModeChange,
  onModelChange,
  onOpenModelSettings,
  onResolveDrop,
  onThinkingLevelChange,
  onSpeedChange,
  onInterrupt,
  onSubmit,
  planState,
}, ref) {
  const t = useT();
  const tRef = useRef(t);
  tRef.current = t;
  const testId = (name: string) => instanceId ? `${instanceId}-${name}` : name;
  const canChangeConfig = canPrompt && configAllowed;
  const configHint = !configAllowed ? t("composer.configBusy") : undefined;
  const planStatus = formatPlanStatus(planState, t);
  const [capabilityHint, setCapabilityHint] = useState<MessageKey | null>(null);
  const [dropActive, setDropActive] = useState(false);
  const [stopping, setStopping] = useState(false);

  useEffect(() => {
    if (!busy) {
      setStopping(false);
    }
  }, [busy]);
  const [draft, setDraft] = useState<ComposerDraft>(EMPTY_DRAFT);
  const [modeMenuOpen, setModeMenuOpen] = useState(false);
  const [mentionOpen, setMentionOpen] = useState(false);
  const [slashState, setSlashState] = useState<SlashSuggestionState | null>(null);
  const isSlashOpenRef = useRef(false);
  const slashMenuRef = useRef<SlashCommandMenuHandle | null>(null);
  const slashCommandsRef = useRef(slashCommands);
  slashCommandsRef.current = slashCommands;
  const catalogRef = useRef(instructionCatalog);
  catalogRef.current = instructionCatalog;
  const slashOpenHandlerRef = useRef(onSlashOpen);
  slashOpenHandlerRef.current = onSlashOpen;
  const previousSlashState = useRef<SlashSuggestionState | null>(null);
  const isComposingRef = useRef(false);
  const isMentionOpenRef = useRef(false);
  const contextSearchDropdownRef = useRef<ContextSearchDropdownHandle | null>(null);
  const draftRef = useRef<ComposerDraft>(EMPTY_DRAFT);
  const modeMenuRef = useRef<HTMLDivElement | null>(null);
  const latestAttachmentHandlersRef = useRef({
    modelCapabilities,
    onAttachFiles,
    onPrepareAttachments,
  });
  latestAttachmentHandlersRef.current = {
    modelCapabilities,
    onAttachFiles,
    onPrepareAttachments,
  };
  const latestContextSearchHandlersRef = useRef({
    onClose: onContextSearchClose,
    onOpen: onContextSearchOpen,
    onQueryChange: onContextSearchQueryChange,
  });
  latestContextSearchHandlersRef.current = {
    onClose: onContextSearchClose,
    onOpen: onContextSearchOpen,
    onQueryChange: onContextSearchQueryChange,
  };
  const latestHandlersRef = useRef({
    canPrompt: canPrompt && (!busy || allowBusyInput) && !submitDisabled && !attachmentsPending,
    onDraftChange,
    onSubmit,
  });
  latestHandlersRef.current = {
    canPrompt: canPrompt && (!busy || allowBusyInput) && !submitDisabled && !attachmentsPending,
    onDraftChange,
    onSubmit,
  };

  const applyDraft = (next: ComposerDraft): boolean => {
    if (sameDraft(draftRef.current, next)) {
      return false;
    }
    setDraft(next);
    draftRef.current = next;
    return true;
  };

  const updateDraft = (next: ComposerDraft) => {
    if (applyDraft(next)) {
      latestHandlersRef.current.onDraftChange(next);
    }
  };

  const mentionSuggestion = useMemo(() =>
    createMentionSuggestion({
      editorHasReference,
      getKeyHandler: () => contextSearchDropdownRef.current?.onKeyDown ?? null,
      isComposing: () => isComposingRef.current,
      onClose: () => {
        isMentionOpenRef.current = false;
        setMentionOpen(false);
        latestContextSearchHandlersRef.current.onClose();
      },
      onOpen: () => {
        isMentionOpenRef.current = true;
        setMentionOpen(true);
        latestContextSearchHandlersRef.current.onOpen();
      },
      onQueryChange: (query) => {
        latestContextSearchHandlersRef.current.onQueryChange(query);
      },
      referenceNodeName: REFERENCE_NODE_NAME,
    }),
  []);

  const slashSuggestion = useMemo(() => createSlashCommandSuggestion({
    getCommands: () => slashCommandsRef.current,
    getCatalog: () => catalogRef.current,
    getKeyHandler: () => slashMenuRef.current?.onKeyDown,
    isComposing: () => isComposingRef.current,
    onState: (next) => {
      if (next && !previousSlashState.current) slashOpenHandlerRef.current?.();
      previousSlashState.current = next;
      isSlashOpenRef.current = next !== null && buildSlashMenuSections(slashCommandsRef.current, next.query, catalogRef.current, next.leading).length > 0;
      setSlashState(next);
    },
  }), []);

  const editor = useEditor({
    immediatelyRender: false,
    extensions: [
      StarterKit.configure({
        blockquote: false,
        bulletList: false,
        code: false,
        codeBlock: false,
        heading: false,
        horizontalRule: false,
        orderedList: false,
      }),
      Placeholder.configure({
        placeholder: () => tRef.current("composer.placeholder"),
      }),
      ReferenceNode,
      InvocationNode,
      mentionSuggestion.extension,
    ],
    editorProps: {
      attributes: {
        "aria-label": t("composer.inputAria"),
        class: "tc-composer__editor",
        "data-testid": testId("composer-input"),
      },
      handleDOMEvents: {
        compositionend: () => {
          isComposingRef.current = false;
          return false;
        },
        compositionstart: () => {
          isComposingRef.current = true;
          return false;
        },
        keydown: (_view, event) => {
          const key = event.key.toLowerCase();
          if (
            (event.metaKey || event.ctrlKey) &&
            (key === "z" || key === "y")
          ) {
            // VS Code forwards webview shortcuts to its workbench. Do not let a
            // Composer undo/redo escape this contentEditable: Tiptap handles the
            // history action locally, while the workbench could undo the active
            // editor or issue a second browser undo.
            event.stopPropagation();
            return false;
          }
          if (isMentionOpenRef.current || isSlashOpenRef.current) {
            return false;
          }
          if (
            event.key === "Enter" &&
            !event.shiftKey &&
            !isComposingRef.current &&
            !event.isComposing &&
            latestHandlersRef.current.canPrompt &&
            draftRef.current.hasContent
          ) {
            event.preventDefault();
            latestHandlersRef.current.onSubmit();
            return true;
          }
          return false;
        },
      },
      handlePaste(view, event) {
        const attachmentFiles: File[] = [];
        const items = event.clipboardData?.items ?? [];
        for (let i = 0; i < items.length; i++) {
          const item = items[i];
          if (item.kind === "file" && isSupportedAttachmentMime(item.type)) {
            const file = item.getAsFile();
            if (file) {
              attachmentFiles.push(file);
            }
          }
        }
        if (attachmentFiles.length > 0) {
          event.preventDefault();
          const hint = buildPasteHint(
            latestAttachmentHandlersRef.current.modelCapabilities,
            attachmentFiles,
          );
          if (hint) {
            setCapabilityHint(hint);
          }
          // Downsample and rasterise here, before the bytes leave the webview. This is
          // the only process in the system with a decoder that can resize during decode,
          // so a 4000x3000 paste is turned into a 192px thumbnail without ever
          // allocating the 48MB full-size bitmap.
          const preparation = Promise.all(
            attachmentFiles.map(async (file) =>
              prepareAttachment({
                bytes: await file.arrayBuffer(),
                filename: file.name || null,
                mimeType: file.type,
                sourcePath: extractFileSourcePath(file),
              }),
            ),
          );
          if (latestAttachmentHandlersRef.current.onPrepareAttachments) {
            // Registration happens synchronously, before any File.arrayBuffer() resolves,
            // so a New Session click can establish a correct click-time cutoff.
            latestAttachmentHandlersRef.current.onPrepareAttachments(preparation);
          } else {
            preparation.then((files) => {
              latestAttachmentHandlersRef.current.onAttachFiles?.(files);
            }).catch(() => undefined);
          }
          preparation.catch(() => {
              // Fallback: paste as text if attachment processing fails
              const text = event.clipboardData?.getData("text/plain");
              if (text) {
                view.dispatch(view.state.tr.insertText(text));
              }
            });
          return true;
        }

        const html = event.clipboardData?.getData("text/html") ?? "";
        if (html.includes("data-tomcat-reference") || html.includes("data-tomcat-instruction")) return false;
        // No images: handle text paste as before
        const text = event.clipboardData?.getData("text/plain");
        if (text === undefined) {
          return false;
        }
        event.preventDefault();
        view.dispatch(view.state.tr.insertText(text));
        return true;
      },
      handleDrop(_view, event) {
        const uris = event.dataTransfer ? extractDropUris(event.dataTransfer) : [];
        if (!uris.length) {
          return false;
        }
        event.preventDefault();
        return true;
      },
    },
    content: initialDraft ? createComposerDocument(initialDraft.segments.length ? initialDraft.segments : [{ type: "text", text: initialDraft.text }]) : {
      content: [
        {
          type: "paragraph",
        },
      ],
      type: "doc",
    },
    onCreate({ editor: nextEditor }) {
      updateDraft(serializeComposerDocument(nextEditor.getJSON()));
    },
    onUpdate({ editor: nextEditor }) {
      updateDraft(serializeComposerDocument(nextEditor.getJSON()));
    },
  });

  useEffect(() => {
    if (!editor) return;
    editor.setOptions({ editorProps: { ...editor.options.editorProps, attributes: {
      "aria-label": t("composer.inputAria"), class: "tc-composer__editor", "data-testid": testId("composer-input"),
    } } });
    // Recompute placeholder decorations without replacing the document, selection or history.
    editor.view.dispatch(editor.state.tr.setMeta("addToHistory", false));
  }, [editor, t]);

  useEffect(() => {
    isSlashOpenRef.current = slashState !== null && buildSlashMenuSections(slashCommands, slashState.query, instructionCatalog, slashState.leading).length > 0;
  }, [slashState, slashCommands, instructionCatalog]);
  useEffect(() => {
    if (!editor) return;
    return slashSuggestion.attach(editor);
  }, [editor, slashSuggestion]);

  useEffect(() => {
    if (!editor) {
      return;
    }
    const handleTestSetComposerValue = (event: Event) => {
      const detail = (event as CustomEvent<{ testId?: string; value?: string | null }>).detail;
      if ((detail?.testId ?? "composer-input") !== testId("composer-input")) {
        return;
      }
      const nextValue = detail?.value ?? "";
      const chain = editor.chain().focus().clearContent(true);
      if (nextValue) {
        chain.insertContent(nextValue);
      }
      chain.focus("end").run();
      updateDraft(serializeComposerDocument(editor.getJSON()));
    };
    window.addEventListener(
      TEST_SET_COMPOSER_VALUE_EVENT,
      handleTestSetComposerValue as EventListener,
    );
    return () => {
      window.removeEventListener(
        TEST_SET_COMPOSER_VALUE_EVENT,
        handleTestSetComposerValue as EventListener,
      );
    };
  }, [editor]);

  useEffect(() => {
    if (!editor) {
      return;
    }
    editor.setEditable(canPrompt);
    if (!canPrompt) {
      mentionSuggestion.close();
      slashSuggestion.close();
      setModeMenuOpen(false);
    }
  }, [canPrompt, editor]);

  useEffect(() => {
    if (!capabilityHint) {
      return;
    }
    const timeout = window.setTimeout(() => {
      setCapabilityHint(null);
    }, 4_000);
    return () => window.clearTimeout(timeout);
  }, [capabilityHint]);

  useEffect(() => {
    if (!modeMenuOpen) {
      return;
    }
    const handleClickOutside = (event: MouseEvent) => {
      if (!(event.target instanceof Node) || modeMenuRef.current?.contains(event.target)) {
        return;
      }
      setModeMenuOpen(false);
    };
    const handleEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setModeMenuOpen(false);
      }
    };
    document.addEventListener("mousedown", handleClickOutside);
    document.addEventListener("keydown", handleEscape);
    return () => {
      document.removeEventListener("mousedown", handleClickOutside);
      document.removeEventListener("keydown", handleEscape);
    };
  }, [modeMenuOpen]);

  useEffect(() => {
    if (!canChangeConfig) setModeMenuOpen(false);
  }, [canChangeConfig]);
  const pickerModels = useMemo(
    () =>
      buildPickerModels({
        activeModelId: modelValue,
        availableModelDetails,
        availableModelReasoningLevels: {
          ...availableModelReasoningLevels,
          ...(supportedReasoningLevels
            ? { [modelValue]: supportedReasoningLevels }
            : {}),
        },
        availableModels,
        selectedModelId: modelValue,
        sessionContextWindow: contextWindowValue,
        sessionThinkingLevel: thinkingLevelValue,
      }),
    [
      availableModelDetails,
      availableModelReasoningLevels,
      availableModels,
      contextWindowValue,
      modelValue,
      supportedReasoningLevels,
      thinkingLevelValue,
    ],
  );

  const handleModePick = (nextMode: "chat" | "plan") => {
    onModeChange(nextMode);
    setModeMenuOpen(false);
  };

  const insertReferences = useCallback((references: readonly WebviewReference[]) => {
    if (!editor) {
      return;
    }
    const seenReferenceIds = new Set<string>();
    const inserted = references.map((reference) => withOccurrence(reference)).filter((reference) => {
      const id = referenceIdentity(reference);
      if (seenReferenceIds.has(id) || editorHasReference(editor, reference)) return false;
      seenReferenceIds.add(id);
      return true;
    });
    if (inserted.length === 0) {
      return;
    }
    editor
      .chain()
      .focus()
      .insertContent(
        inserted.flatMap((reference) => [
          {
            attrs: reference,
            type: REFERENCE_NODE_NAME,
          },
          {
            text: " ",
            type: "text",
          },
        ]),
      )
      .run();
    // A system picker can add N references at once. Persist its post-transaction
    // document only once; emitting N intermediate drafts allows an older prefix to
    // arrive after the complete draft and overwrite it in the extension host.
    updateDraft(serializeComposerDocument(editor.getJSON()));
  }, [editor]);

  useImperativeHandle(ref, () => ({
    clear() {
      if (!editor) {
        updateDraft(EMPTY_DRAFT);
        return;
      }
      editor.commands.clearContent(true);
      editor.commands.focus("end");
      updateDraft(serializeComposerDocument(editor.getJSON()));
    },
    closeMention() {
      mentionSuggestion.close();
      slashSuggestion.close();
    },
    getDraft() {
      return draftRef.current;
    },
    insertReference(reference: WebviewReference) {
      insertReferences([reference]);
    },
    insertReferences(references: readonly WebviewReference[]) {
      insertReferences(references);
    },
    replaceDraft(nextDraft: ComposerDraft) {
      if (!editor) {
        applyDraft(nextDraft);
        return;
      }
      const segments = nextDraft.segments.length
        ? nextDraft.segments
        : nextDraft.text
          ? [{ text: nextDraft.text, type: "text" } satisfies WebviewMessageSegment]
          : [];
      editor.commands.setContent(createComposerDocument(segments), { emitUpdate: false });
      editor.commands.focus("end");
      applyDraft(serializeComposerDocument(editor.getJSON()));
    },
  }), [editor, insertReferences, mentionSuggestion, slashSuggestion]);

  const handleDragOver = (event: DragEvent<HTMLDivElement>) => {
    if (!canPrompt) {
      return;
    }
    event.preventDefault();
    setDropActive(true);
  };

  const handleDragEnter = (event: DragEvent<HTMLDivElement>) => {
    if (!canPrompt) {
      return;
    }
    event.preventDefault();
  };

  const handleDragLeave = (event: DragEvent<HTMLDivElement>) => {
    if (event.currentTarget.contains(event.relatedTarget as Node | null)) {
      return;
    }
    setDropActive(false);
  };

  const handleDrop = (event: DragEvent<HTMLDivElement>) => {
    if (!canPrompt) {
      return;
    }
    event.preventDefault();
    setDropActive(false);
    const uris = extractDropUris(event.dataTransfer);
    if (uris.length) {
      const hint = buildDropHint(modelCapabilities, uris);
      if (hint) {
        setCapabilityHint(hint);
      }
      onResolveDrop(uris);
    }
  };

  const handleDragEnd = () => {
    setDropActive(false);
  };

  const handlePickContext = () => {
    const hint = buildPickerHint(modelCapabilities);
    if (hint) {
      setCapabilityHint(hint);
    }
    onPickContext();
  };

  const warningNotice: ComposerNotice | null = capabilityHint
    ? {
        id: "capability",
        text: t(capabilityHint),
        tone: "warning",
      }
    : null;
  const dragNotice: ComposerNotice | null = !hideDragHint && !warningNotice && canPrompt
    ? dropActive
      ? {
          id: "drag",
          text: t("composer.dragReady"),
          tone: "active",
        }
      : {
          id: "drag",
          text: t("composer.dragTip"),
          tone: "info",
        }
    : null;
  const planNotice: ComposerNotice | null = !warningNotice && planStatus
    ? {
        id: "plan",
        text: planStatus,
        tone: "plan",
      }
    : null;
  const hasNotice = Boolean(warningNotice || dragNotice || planNotice || commandPending);
  // Content presence selects the action; validation only enables/disables Send.
  // Keep the legacy Stop-only behavior when an older server cannot queue input.
  const hasInput = draft.hasContent || hasAttachments || attachmentsPending;
  const showStop = busy && (!allowBusyInput || !hasInput);
  const submitName = submitAriaLabel ?? t(busy ? "composer.queue" : "composer.send");

  return (
    <section className="tc-composer" aria-label={t("composer.aria")} data-testid={testId("composer")} onKeyDown={(event) => {
      if (event.key === "Escape" && !event.defaultPrevented && !event.nativeEvent.isComposing && !mentionOpen && !slashState && !modeMenuOpen && !event.currentTarget.querySelector('[aria-expanded="true"]')) {
        event.preventDefault(); onCancelEdit?.();
      }
    }}>
      <div
        className={`tc-composer__surface${dropActive ? " tc-composer__surface--drop-active" : ""}`}
        data-testid={testId("composer-surface")}
        onDragEnd={handleDragEnd}
        onDragEnter={handleDragEnter}
        onDragLeave={handleDragLeave}
        onDragOver={handleDragOver}
        onDrop={handleDrop}
      >
        {header ? <header className="tc-composer__header">{header}</header> : null}
        <div className="tc-composer__body">
        {hasNotice ? (
          <div className="tc-composer__notices" role="status" aria-live="polite" data-testid={testId("composer-notices")}>
            {commandPending && <span className="tc-notice tc-notice--info" data-testid={testId("composer-notice-command")}>{t("composer.commandPending")}</span>}
            {warningNotice ? (
              <span className="tc-notice tc-notice--warning" data-testid={testId("composer-notice-capability")}>
                {warningNotice.text}
              </span>
            ) : (
              <>
                {dragNotice ? (
                  <span
                    aria-hidden="true"
                    className={`tc-notice tc-notice--${dragNotice.tone} tc-notice--left`}
                    data-testid={testId("composer-notice-drag")}
                  >
                    {dragNotice.tone === "info" ? (
                      <>
                        <strong className="tc-notice__tip">{t("composer.tip")}</strong> {dragNotice.text}
                      </>
                    ) : (
                      dragNotice.text
                    )}
                  </span>
                ) : null}
                {planNotice ? (
                  <span className="tc-notice tc-notice--plan tc-notice--right" data-testid={testId("composer-notice-plan")}>
                    {planNotice.text}
                  </span>
                ) : null}
              </>
            )}
          </div>
        ) : null}
        {beforeEditor}
        <ContextSearchDropdown
          ref={contextSearchDropdownRef}
          loading={contextSearchLoading}
          matches={contextSearchMatches}
          onSelect={(match) => {
            mentionSuggestion.command(match);
          }}
          open={mentionOpen}
          query={contextSearchQuery}
          truncated={contextSearchTruncated}
        />
        <SlashCommandMenu ref={slashMenuRef} sections={buildSlashMenuSections(slashCommands, slashState?.query ?? "", instructionCatalog, slashState?.leading ?? true, t)} query={slashState?.query ?? ""} open={slashState !== null} onSelect={(item) => slashSuggestion.command(item)} onClose={() => slashSuggestion.close()} />
        <EditorContent editor={editor} />
        <div className="tc-composer__bar" data-testid={testId("composer-bar")}>
          <button
            aria-label={t("composer.addContext")}
            className="tc-icon-button"
            data-testid={testId("attachment-add")}
            disabled={!canPrompt}
            onClick={handlePickContext}
            title={t("composer.addContext")}
            type="button"
          >
            +
          </button>
          <span aria-hidden="true" className="tc-composer__bar-sep">
            |
          </span>

          <div
            className="tc-field tc-field--compact tc-field--dropdown tc-field--mode"
            ref={modeMenuRef}
            title={configHint}
          >
            <span>{t("composer.mode")}</span>
            <button
              aria-expanded={modeMenuOpen}
              aria-label={t("composer.modeAria")}
              className="tc-topbar__trigger tc-topbar__trigger--compact"
              data-testid={testId("mode-select")}
              disabled={!canChangeConfig}
              onClick={() => {
                setModeMenuOpen((value) => !value);
              }}
              type="button"
            >
              <span className="tc-topbar__trigger-label">{modeLabel(modeValue, t)}</span>
              <span className="tc-topbar__caret" aria-hidden="true">
                {modeMenuOpen ? "▴" : "▾"}
              </span>
            </button>
            {modeMenuOpen ? (
              <div className="tc-session-dropdown tc-composer-dropdown" data-testid={testId("mode-dropdown")}>
                {MODE_OPTIONS.map((option) => {
                  const isActive = option.value === modeValue;
                  return (
                    <button
                      aria-current={isActive ? "true" : undefined}
                      className={`tc-session-item${isActive ? " tc-session-item--active" : ""}`}
                      data-testid={testId("mode-option")}
                      key={option.value}
                      onClick={() => handleModePick(option.value)}
                      type="button"
                    >
                      <span className="tc-session-item__title">{t(option.labelKey)}</span>
                    </button>
                  );
                })}
              </div>
            ) : null}
          </div>
          <span aria-hidden="true" className="tc-composer__bar-sep">
            |
          </span>

          <div className="tc-field tc-field--compact tc-field--dropdown tc-field--model" title={configHint}>
            <span>{t("model.label")}</span>
            <ModelPicker
              className="tc-composer-model-picker"
              disabled={!canChangeConfig}
              // The VS Code host E2E harness still drives this compatibility trigger.
              legacyThinkingTriggerTestId={testId("thinking-level-select")}
              testId={testId("model-select")}
              dropdownTestId={testId("model-dropdown")}
              optionTestId={testId("model-option")}
              models={pickerModels}
              onOpenModelSettings={onOpenModelSettings}
              onSelectContextWindow={(selectedModelId, contextWindow) => {
                setModeMenuOpen(false);
                onContextWindowChange(selectedModelId, contextWindow);
              }}
              onSelectModel={(selectedModelId) => {
                setModeMenuOpen(false);
                onModelChange(selectedModelId);
              }}
              onSelectSpeed={(selectedModelId, speed) => {
                setModeMenuOpen(false);
                onSpeedChange(selectedModelId, speed);
              }}
              onSelectThinkingLevel={(selectedModelId, level) => {
                setModeMenuOpen(false);
                onThinkingLevelChange(selectedModelId, level);
              }}
              selectedModelId={modelValue}
            />
          </div>

          <span className="tc-composer__context" data-testid={testId("context-ratio")}>
            {contextLabel}
          </span>

          <button
            aria-label={showStop ? t(stopping ? "composer.stopping" : "composer.stop") : submitName}
            title={showStop ? t(stopping ? "composer.stopping" : "composer.stopTooltip") : submitAriaLabel ?? t(busy ? "composer.queueTooltip" : "composer.sendTooltip")}
            className="tc-send-button"
            data-testid={testId(showStop ? "stop-button" : "send-button")}
            disabled={showStop ? !canInterrupt || stopping : !draft.hasContent || !canPrompt || submitDisabled || attachmentsPending}
            onClick={
              showStop
                ? () => {
                    if (stopping) return;
                    setStopping(true);
                    onInterrupt?.();
                  }
                : onSubmit
            }
            type="button"
          >
            {showStop && stopping ? (
              t("composer.stopping")
            ) : showStop ? (
              <span aria-hidden="true" className="tc-stop-square" data-testid={testId("stop-glyph")} />
            ) : (
              "↑"
            )}
          </button>
        </div>
        </div>
      </div>
    </section>
  );
});
