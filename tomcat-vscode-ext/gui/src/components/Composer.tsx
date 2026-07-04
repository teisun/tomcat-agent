import {
  forwardRef,
  useEffect,
  useImperativeHandle,
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
  WebviewPlanState,
  WebviewMessageSegment,
  WebviewReference,
} from "../types";
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

export interface ComposerDraft {
  hasContent: boolean;
  segments: WebviewMessageSegment[];
  text: string;
}

export interface ComposerHandle {
  clear(): void;
  getDraft(): ComposerDraft;
  insertReference(reference: WebviewReference): void;
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
  canPrompt: boolean;
  contextLabel: string;
  modelCapabilities?: string[] | undefined;
  modeValue: "chat" | "plan";
  modelValue: string;
  thinkingLevelValue: "" | "high" | "low" | "medium" | "xhigh";
  onPickContext(): void;
  onDraftChange(draft: ComposerDraft): void;
  onModeChange(value: "chat" | "plan"): void;
  onModelChange(value: string): void;
  onResolveDrop(uris: string[]): void;
  onThinkingLevelChange(value: "high" | "low" | "medium" | "xhigh" | ""): void;
  onInterrupt?(): void;
  onSubmit(): void;
  planState?: WebviewPlanState | null;
}

export const Composer = forwardRef<ComposerHandle, ComposerProps>(function Composer({
  availableModels,
  busy = false,
  canPrompt,
  contextLabel,
  modelCapabilities,
  modeValue,
  modelValue,
  thinkingLevelValue,
  onPickContext,
  onDraftChange,
  onModeChange,
  onModelChange,
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
  const isComposingRef = useRef(false);
  const draftRef = useRef<ComposerDraft>(EMPTY_DRAFT);
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
  }), [editor]);

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
        <EditorContent editor={editor} />
        {canPrompt && (dropActive || !draft.hasContent) ? (
          <div
            aria-hidden="true"
            className={`tc-composer__hint${dropActive ? " tc-composer__hint--active" : ""}`}
            data-testid="composer-dnd-hint"
          >
            {dropActive ? "松手加入上下文" : "拖文件请按住 Shift"}
          </div>
        ) : null}
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

          <label className="tc-field tc-field--compact tc-field--mode">
            <span>Mode</span>
            <select
              aria-label="Tomcat chat mode"
              data-testid="mode-select"
              disabled={!canPrompt}
              onChange={(event) => onModeChange(event.target.value as "chat" | "plan")}
              value={modeValue}
            >
              <option value="chat">Chat</option>
              <option value="plan">Plan</option>
            </select>
          </label>

          <label className="tc-field tc-field--compact tc-field--model">
            <span>Model</span>
            <select
              aria-label="Tomcat model"
              data-testid="model-select"
              disabled={!canPrompt || !availableModels.length}
              onChange={(event) => onModelChange(event.target.value)}
              value={modelValue}
            >
              <option value="">Select model</option>
              {availableModels.map((model) => (
                <option key={model} value={model}>
                  {model}
                </option>
              ))}
            </select>
          </label>

          <label className="tc-field tc-field--compact tc-field--effort">
            <span>Effort</span>
            <select
              aria-label="Tomcat reasoning effort"
              data-testid="thinking-level-select"
              disabled={!canPrompt || !modelValue}
              onChange={(event) =>
                onThinkingLevelChange(
                  event.target.value as "high" | "low" | "medium" | "xhigh" | "",
                )
              }
              value={thinkingLevelValue}
            >
              <option value="">Effort</option>
              <option value="low">Low</option>
              <option value="medium">Medium</option>
              <option value="high">High</option>
              <option value="xhigh">Xhigh</option>
            </select>
          </label>

          <span className="tc-composer__context" data-testid="context-ratio">
            {contextLabel}
          </span>

          <button
            aria-label={busy ? "Stop" : "Send prompt"}
            className="tc-send-button"
            data-testid={busy ? "stop-button" : "send-button"}
            disabled={busy ? false : !draft.hasContent || !canPrompt}
            onClick={busy ? onInterrupt : onSubmit}
            type="button"
          >
            {busy ? <span aria-hidden="true" className="codicon codicon-stop" /> : "↑"}
          </button>
        </div>
      </div>
      {planStatus ? (
        <div className="tc-composer__footer" data-testid="composer-footer">
          <span
            className="tc-chip tc-composer__plan-status"
            data-testid="composer-plan-status-footer"
          >
            {planStatus}
          </span>
        </div>
      ) : null}
      {capabilityHint ? (
        <div className="tc-composer__footer">
          <span
            className="tc-chip tc-chip--warning tc-composer__capability-hint"
            data-testid="composer-capability-hint"
          >
            {capabilityHint}
          </span>
        </div>
      ) : null}
    </section>
  );
});
