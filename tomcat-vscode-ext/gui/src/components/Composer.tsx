import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
  type DragEvent,
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

import { referenceIdentity } from "../contextReferences";
import type {
  ContextSearchMatch,
  WebviewPlanState,
  WebviewMessageSegment,
  WebviewReference,
} from "../types";
import {
  ContextSearchDropdown,
  type ContextSearchDropdownHandle,
} from "./ContextSearchDropdown";
import { createMentionSuggestion } from "./mentionSuggestion";
import { ReferenceChip } from "./ReferenceChip";

function formatPlanStatus(planState?: WebviewPlanState | null): string | null {
  if (!planState || planState === "chat") {
    return null;
  }
  return `Plan: ${planState}`;
}

const REFERENCE_NODE_NAME = "reference";
const DROP_URI_SCHEMES = /^(file|vscode-file|vscode-remote):/i;
const DEFAULT_PROMPT_PLACEHOLDER = "Message Tomcat (Enter to send, Shift+Enter for newline)";
const IMAGE_ATTACHMENT_EXTENSIONS = new Set([".gif", ".jpeg", ".jpg", ".png", ".webp"]);
const TEST_SET_COMPOSER_VALUE_EVENT = "tomcat:test:set-composer-value";
const MODE_OPTIONS = [
  { label: "Chat", value: "chat" },
  { label: "Plan", value: "plan" },
] as const;
const THINKING_LEVEL_OPTIONS = [
  { label: "Effort", value: "" },
  { label: "Low", value: "low" },
  { label: "Medium", value: "medium" },
  { label: "High", value: "high" },
  { label: "Xhigh", value: "xhigh" },
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

function buildPickerHint(capabilities?: string[]): string | null {
  if (!capabilities) {
    return null;
  }
  const supportsVision = hasCapability(capabilities, "vision");
  const supportsFiles = hasCapability(capabilities, "files");
  if (supportsVision && supportsFiles) {
    return null;
  }
  if (!supportsVision && !supportsFiles) {
    return "当前模型不支持图片/PDF 附件；图片/PDF 会先加入待发送列表，发送时会提示切换模型。";
  }
  if (!supportsVision) {
    return "当前模型不支持图片附件；图片会先加入待发送列表，发送时会提示切换模型。";
  }
  return "当前模型不支持 PDF 附件；PDF 会先加入待发送列表，发送时会提示切换模型。";
}

function buildDropHint(capabilities: string[] | undefined, uris: string[]): string | null {
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
    return "当前模型不支持图片/PDF 附件；拖入后会先加入待发送列表，发送时会提示切换模型。";
  }
  if (includesImage && !supportsVision) {
    return "当前模型不支持图片附件；拖入后会先加入待发送列表，发送时会提示切换模型。";
  }
  if (includesPdf && !supportsFiles) {
    return "当前模型不支持 PDF 附件；拖入后会先加入待发送列表，发送时会提示切换模型。";
  }
  return null;
}

function modeLabel(value: "chat" | "plan"): string {
  return MODE_OPTIONS.find((option) => option.value === value)?.label ?? "Chat";
}

function thinkingLevelLabel(value: "" | "high" | "low" | "medium" | "xhigh"): string {
  return THINKING_LEVEL_OPTIONS.find((option) => option.value === value)?.label ?? "Effort";
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
      (segment) => segment.type === "reference" || segment.text.trim().length > 0,
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
      attrs: segment,
      type: REFERENCE_NODE_NAME,
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

interface ComposerProps {
  availableModels: string[];
  busy?: boolean;
  canInterrupt: boolean;
  canPrompt: boolean;
  contextSearchLoading: boolean;
  contextSearchMatches: ContextSearchMatch[];
  contextSearchQuery: string;
  contextSearchTruncated: boolean;
  contextLabel: string;
  modelCapabilities?: string[] | undefined;
  modeValue: "chat" | "plan";
  modelValue: string;
  onContextSearchClose(): void;
  onContextSearchOpen(): void;
  onContextSearchQueryChange(query: string): void;
  thinkingLevelValue: "" | "high" | "low" | "medium" | "xhigh";
  onPickContext(): void;
  onDraftChange(draft: ComposerDraft): void;
  onModeChange(value: "chat" | "plan"): void;
  onModelChange(value: string): void;
  onOpenModelSettings?(): void;
  onResolveDrop(uris: string[]): void;
  onThinkingLevelChange(value: "high" | "low" | "medium" | "xhigh" | ""): void;
  onInterrupt?(): void;
  onSubmit(): void;
  planState?: WebviewPlanState | null;
}

export const Composer = forwardRef<ComposerHandle, ComposerProps>(function Composer({
  availableModels,
  busy = false,
  canInterrupt,
  canPrompt,
  contextSearchLoading,
  contextSearchMatches,
  contextSearchQuery,
  contextSearchTruncated,
  contextLabel,
  modelCapabilities,
  modeValue,
  modelValue,
  onContextSearchClose,
  onContextSearchOpen,
  onContextSearchQueryChange,
  thinkingLevelValue,
  onPickContext,
  onDraftChange,
  onModeChange,
  onModelChange,
  onOpenModelSettings,
  onResolveDrop,
  onThinkingLevelChange,
  onInterrupt,
  onSubmit,
  planState,
}, ref) {
  const planStatus = formatPlanStatus(planState);
  const [capabilityHint, setCapabilityHint] = useState<string | null>(null);
  const [dropActive, setDropActive] = useState(false);
  const [draft, setDraft] = useState<ComposerDraft>(EMPTY_DRAFT);
  const [modelMenuOpen, setModelMenuOpen] = useState(false);
  const [modeMenuOpen, setModeMenuOpen] = useState(false);
  const [effortMenuOpen, setEffortMenuOpen] = useState(false);
  const [mentionOpen, setMentionOpen] = useState(false);
  const isComposingRef = useRef(false);
  const isMentionOpenRef = useRef(false);
  const contextSearchDropdownRef = useRef<ContextSearchDropdownHandle | null>(null);
  const draftRef = useRef<ComposerDraft>(EMPTY_DRAFT);
  const modelMenuRef = useRef<HTMLDivElement | null>(null);
  const modeMenuRef = useRef<HTMLDivElement | null>(null);
  const effortMenuRef = useRef<HTMLDivElement | null>(null);
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
    canPrompt,
    onDraftChange,
    onSubmit,
  });
  latestHandlersRef.current = {
    canPrompt,
    onDraftChange,
    onSubmit,
  };

  const updateDraft = (next: ComposerDraft) => {
    setDraft(next);
    draftRef.current = next;
    latestHandlersRef.current.onDraftChange(next);
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
        placeholder: DEFAULT_PROMPT_PLACEHOLDER,
      }),
      ReferenceNode,
      mentionSuggestion.extension,
    ],
    editorProps: {
      attributes: {
        "aria-label": "Tomcat prompt",
        class: "tc-composer__editor",
        "data-testid": "composer-input",
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
          if (isMentionOpenRef.current) {
            return false;
          }
          if (
            event.key === "Enter" &&
            !event.shiftKey &&
            !isComposingRef.current &&
            !event.isComposing &&
            latestHandlersRef.current.canPrompt
          ) {
            event.preventDefault();
            latestHandlersRef.current.onSubmit();
            return true;
          }
          return false;
        },
      },
      handlePaste(view, event) {
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
    content: {
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
    if (!editor) {
      return;
    }
    const handleTestSetComposerValue = (event: Event) => {
      const detail = (event as CustomEvent<{ testId?: string; value?: string | null }>).detail;
      if (detail?.testId && detail.testId !== "composer-input") {
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
    if (!modelMenuOpen && !modeMenuOpen && !effortMenuOpen) {
      return;
    }
    const refs = [modelMenuRef, modeMenuRef, effortMenuRef];
    const handleClickOutside = (event: MouseEvent) => {
      if (!(event.target instanceof Node)) {
        return;
      }
      if (refs.some((ref) => ref.current?.contains(event.target))) {
        return;
      }
      setModelMenuOpen(false);
      setModeMenuOpen(false);
      setEffortMenuOpen(false);
    };
    const handleEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setModelMenuOpen(false);
        setModeMenuOpen(false);
        setEffortMenuOpen(false);
      }
    };
    document.addEventListener("mousedown", handleClickOutside);
    document.addEventListener("keydown", handleEscape);
    return () => {
      document.removeEventListener("mousedown", handleClickOutside);
      document.removeEventListener("keydown", handleEscape);
    };
  }, [effortMenuOpen, modeMenuOpen, modelMenuOpen]);

  const useNativeModelSelect = !onOpenModelSettings;
  const canOpenModelMenu = canPrompt && (availableModels.length > 0 || Boolean(onOpenModelSettings));
  const canOpenModeMenu = canPrompt;
  const canOpenEffortMenu = canPrompt && Boolean(modelValue);

  const handleModelPick = (nextModel: string) => {
    onModelChange(nextModel);
    setModelMenuOpen(false);
    setModeMenuOpen(false);
    setEffortMenuOpen(false);
  };

  const handleModePick = (nextMode: "chat" | "plan") => {
    onModeChange(nextMode);
    setModelMenuOpen(false);
    setModeMenuOpen(false);
    setEffortMenuOpen(false);
  };

  const handleThinkingLevelPick = (
    nextLevel: "high" | "low" | "medium" | "xhigh" | "",
  ) => {
    onThinkingLevelChange(nextLevel);
    setModelMenuOpen(false);
    setModeMenuOpen(false);
    setEffortMenuOpen(false);
  };

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
    },
    getDraft() {
      return draftRef.current;
    },
    insertReference(reference: WebviewReference) {
      if (!editor || editorHasReference(editor, reference)) {
        return;
      }
      editor
        .chain()
        .focus()
        .insertContent([
          {
            attrs: reference,
            type: REFERENCE_NODE_NAME,
          },
          {
            text: " ",
            type: "text",
          },
        ])
        .run();
      updateDraft(serializeComposerDocument(editor.getJSON()));
    },
    replaceDraft(nextDraft: ComposerDraft) {
      if (!editor) {
        updateDraft(nextDraft);
        return;
      }
      const segments = nextDraft.segments.length
        ? nextDraft.segments
        : nextDraft.text
          ? [{ text: nextDraft.text, type: "text" } satisfies WebviewMessageSegment]
          : [];
      editor.commands.setContent(createComposerDocument(segments), false);
      editor.commands.focus("end");
      updateDraft(serializeComposerDocument(editor.getJSON()));
    },
  }), [editor, mentionSuggestion]);

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
        text: capabilityHint,
        tone: "warning",
      }
    : null;
  const dragNotice: ComposerNotice | null = !warningNotice && canPrompt
    ? dropActive
      ? {
          id: "drag",
          text: "松手加入上下文",
          tone: "active",
        }
      : !draft.hasContent
        ? {
            id: "drag",
            text: "拖文件请按住 Shift",
            tone: "info",
          }
        : null
    : null;
  const planNotice: ComposerNotice | null = !warningNotice && planStatus
    ? {
        id: "plan",
        text: planStatus,
        tone: "plan",
      }
    : null;
  const hasNotice = Boolean(warningNotice || dragNotice || planNotice);

  return (
    <section className="tc-composer" aria-label="prompt" data-testid="composer">
      <div
        className={`tc-composer__surface${dropActive ? " tc-composer__surface--drop-active" : ""}`}
        data-testid="composer-surface"
        onDragEnd={handleDragEnd}
        onDragEnter={handleDragEnter}
        onDragLeave={handleDragLeave}
        onDragOver={handleDragOver}
        onDrop={handleDrop}
      >
        {hasNotice ? (
          <div className="tc-composer__notices" role="status" aria-live="polite" data-testid="composer-notices">
            {warningNotice ? (
              <span className="tc-notice tc-notice--warning" data-testid="composer-notice-capability">
                {warningNotice.text}
              </span>
            ) : (
              <>
                {dragNotice ? (
                  <span
                    aria-hidden="true"
                    className={`tc-notice tc-notice--${dragNotice.tone} tc-notice--left`}
                    data-testid="composer-notice-drag"
                  >
                    {dragNotice.tone === "info" ? (
                      <>
                        <strong className="tc-notice__tip">Tip:</strong> {dragNotice.text}
                      </>
                    ) : (
                      dragNotice.text
                    )}
                  </span>
                ) : null}
                {planNotice ? (
                  <span className="tc-notice tc-notice--plan tc-notice--right" data-testid="composer-notice-plan">
                    {planNotice.text}
                  </span>
                ) : null}
              </>
            )}
          </div>
        ) : null}
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
        <EditorContent editor={editor} />
        <div className="tc-composer__bar" data-testid="composer-bar">
          <button
            aria-label="添加文件/文件夹/图片"
            className="tc-icon-button"
            data-testid="attachment-add"
            disabled={!canPrompt}
            onClick={handlePickContext}
            title="添加文件/文件夹/图片"
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
          >
            <span>Mode</span>
            <button
              aria-expanded={modeMenuOpen}
              aria-label="Tomcat chat mode"
              className="tc-topbar__trigger tc-topbar__trigger--compact"
              data-testid="mode-select"
              disabled={!canOpenModeMenu}
              onClick={() => {
                setModelMenuOpen(false);
                setEffortMenuOpen(false);
                setModeMenuOpen((value) => !value);
              }}
              type="button"
            >
              <span className="tc-topbar__trigger-label">{modeLabel(modeValue)}</span>
              <span className="tc-topbar__caret" aria-hidden="true">
                {modeMenuOpen ? "▴" : "▾"}
              </span>
            </button>
            {modeMenuOpen ? (
              <div className="tc-session-dropdown tc-composer-dropdown" data-testid="mode-dropdown">
                {MODE_OPTIONS.map((option) => {
                  const isActive = option.value === modeValue;
                  return (
                    <button
                      aria-current={isActive ? "true" : undefined}
                      className={`tc-session-item${isActive ? " tc-session-item--active" : ""}`}
                      data-testid="mode-option"
                      key={option.value}
                      onClick={() => handleModePick(option.value)}
                      type="button"
                    >
                      <span className="tc-session-item__title">{option.label}</span>
                    </button>
                  );
                })}
              </div>
            ) : null}
          </div>
          <span aria-hidden="true" className="tc-composer__bar-sep">
            |
          </span>

          <div
            className="tc-field tc-field--compact tc-field--dropdown tc-field--model"
            ref={useNativeModelSelect ? null : modelMenuRef}
          >
            <span>Model</span>
            {useNativeModelSelect ? (
              <select
                aria-label="Tomcat model"
                data-testid="model-select"
                disabled={!canPrompt || availableModels.length === 0}
                onChange={(event) => onModelChange(event.target.value)}
                value={modelValue}
              >
                {availableModels.length === 0 ? (
                  <option value="">No ready models</option>
                ) : null}
                {availableModels.map((model) => (
                  <option key={model} value={model}>
                    {model}
                  </option>
                ))}
              </select>
            ) : (
              <>
                <button
                  aria-expanded={modelMenuOpen}
                  aria-label="Tomcat model"
                  className="tc-topbar__trigger tc-topbar__trigger--compact"
                  data-testid="model-select"
                  disabled={!canOpenModelMenu}
                  onClick={() => {
                    setModeMenuOpen(false);
                    setEffortMenuOpen(false);
                    setModelMenuOpen((value) => !value);
                  }}
                  type="button"
                >
                  <span className="tc-topbar__trigger-label">
                    {modelValue || (availableModels.length ? "Select model" : "Add models")}
                  </span>
                  <span className="tc-topbar__caret" aria-hidden="true">
                    {modelMenuOpen ? "▴" : "▾"}
                  </span>
                </button>
                {modelMenuOpen ? (
                  <div className="tc-session-dropdown tc-model-dropdown" data-testid="model-dropdown">
                    {availableModels.length > 0 ? (
                      availableModels.map((model) => {
                        const isActive = model === modelValue;
                        return (
                          <button
                            aria-current={isActive ? "true" : undefined}
                            className={`tc-session-item${isActive ? " tc-session-item--active" : ""}`}
                            data-testid="model-option"
                            key={model}
                            onClick={() => handleModelPick(model)}
                            type="button"
                          >
                            <span className="tc-session-item__title">{model}</span>
                          </button>
                        );
                      })
                    ) : (
                      <div className="tc-session-dropdown__empty">No ready models</div>
                    )}
                    <div className="tc-model-dropdown__divider" />
                    <button
                      className="tc-session-item tc-model-dropdown__footer"
                      data-testid="model-open-settings"
                      onClick={() => {
                        setModelMenuOpen(false);
                        setModeMenuOpen(false);
                        setEffortMenuOpen(false);
                        onOpenModelSettings();
                      }}
                      type="button"
                    >
                      <span className="tc-session-item__title">Add Models...</span>
                    </button>
                  </div>
                ) : null}
              </>
            )}
          </div>
          <span aria-hidden="true" className="tc-composer__bar-sep">
            |
          </span>

          <div
            className="tc-field tc-field--compact tc-field--dropdown tc-field--effort"
            ref={effortMenuRef}
          >
            <span>Effort</span>
            <button
              aria-expanded={effortMenuOpen}
              aria-label="Tomcat reasoning effort"
              className="tc-topbar__trigger tc-topbar__trigger--compact"
              data-testid="thinking-level-select"
              disabled={!canOpenEffortMenu}
              onClick={() => {
                setModelMenuOpen(false);
                setModeMenuOpen(false);
                setEffortMenuOpen((value) => !value);
              }}
              type="button"
            >
              <span className="tc-topbar__trigger-label">
                {thinkingLevelLabel(thinkingLevelValue)}
              </span>
              <span className="tc-topbar__caret" aria-hidden="true">
                {effortMenuOpen ? "▴" : "▾"}
              </span>
            </button>
            {effortMenuOpen ? (
              <div
                className="tc-session-dropdown tc-composer-dropdown"
                data-testid="thinking-level-dropdown"
              >
                {THINKING_LEVEL_OPTIONS.map((option) => {
                  const isActive = option.value === thinkingLevelValue;
                  return (
                    <button
                      aria-current={isActive ? "true" : undefined}
                      className={`tc-session-item${isActive ? " tc-session-item--active" : ""}`}
                      data-testid="thinking-level-option"
                      key={option.label}
                      onClick={() => handleThinkingLevelPick(option.value)}
                      type="button"
                    >
                      <span className="tc-session-item__title">{option.label}</span>
                    </button>
                  );
                })}
              </div>
            ) : null}
          </div>

          <span className="tc-composer__context" data-testid="context-ratio">
            {contextLabel}
          </span>

          <button
            aria-label={busy ? "Stop" : "Send prompt"}
            className="tc-send-button"
            data-testid={busy ? "stop-button" : "send-button"}
            disabled={busy ? !canInterrupt : !draft.hasContent || !canPrompt}
            onClick={busy ? onInterrupt : onSubmit}
            type="button"
          >
            {busy ? <span aria-hidden="true" className="codicon codicon-stop" /> : "↑"}
          </button>
        </div>
      </div>
    </section>
  );
});
