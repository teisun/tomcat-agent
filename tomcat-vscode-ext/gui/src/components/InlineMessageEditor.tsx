import { useEffect, useRef, useState } from "react";
import { freshOccurrences } from "../../../src/shared/composerOccurrences";
import { isPreviewRewindResponse } from "../../../src/shared/messageEditProtocol";
import type { PreviewRewindResponse } from "../../../src/serveClient/wire";
import type {
  ContextSearchMatch,
  VsCodeApiLike,
  WebviewIntent,
  WebviewMessageBlock,
  WebviewPendingAttachment,
  WebviewReference,
} from "../types";
import type { ComposerDraft, ComposerHandle } from "./Composer";
import { ComposerSurface, type ComposerSurfaceProps } from "./ComposerSurface";
import { EditConfirmDialog } from "./EditConfirmDialog";
import { ImageLightbox, type ZoomedImage } from "./ImageLightbox";
import "./inlineMessageEditing.css";

const id = () => `edit-${Date.now()}-${Math.random().toString(36).slice(2)}`;

export function InlineMessageEditor({
  message,
  sessionId,
  composerProps,
  hasWrites,
  busy = false,
  vscodeApi,
  onClose,
}: {
  message: WebviewMessageBlock;
  sessionId: string;
  composerProps: ComposerSurfaceProps;
  hasWrites: boolean;
  busy?: boolean;
  vscodeApi: VsCodeApiLike;
  onClose(): void;
}) {
  const editor = useRef<ComposerHandle>(null);
  const root = useRef<HTMLDivElement>(null);
  const inside = useRef(false);
  const mounted = useRef(true);
  const [initialDraft] = useState<ComposerDraft>(() => ({
    hasContent: true,
    text: message.text,
    segments: freshOccurrences(
      message.segments?.length ? message.segments : [{ type: "text", text: message.text }],
    ),
  }));
  const [attachments, setAttachments] = useState<WebviewPendingAttachment[]>(() =>
    (message.attachments ?? []).map((a) => ({ ...a, label: a.filename })),
  );
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [zoomedImage, setZoomedImage] = useState<ZoomedImage | null>(null);
  const [preview, setPreview] = useState<PreviewRewindResponse | null>(null);
  const [workCount, setWorkCount] = useState(0);
  const work = useRef(new Set<string>());
  const request = useRef<{ id: string; kind: "preview" | "send" } | null>(null);
  const frozen = useRef<{
    draft: ComposerDraft;
    attachments: WebviewPendingAttachment[];
    busy: boolean;
  } | null>(null);
  const [search, setSearch] = useState({
    query: "",
    matches: [] as ContextSearchMatch[],
    loading: false,
    truncated: false,
  });
  const searchRequest = useRef<string | null>(null);
  const searchTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const protectedRef = useRef(false);
  protectedRef.current = pending || !!preview || !!zoomedImage;
  const closeRef = useRef(onClose);
  closeRef.current = onClose;

  function post(type: WebviewIntent["type"], data: Record<string, unknown>, messageId = id()) {
    vscodeApi.postMessage({ type, data, messageId } as WebviewIntent);
    return messageId;
  }

  function beginWork() {
    const key = id();
    work.current.add(key);
    setWorkCount(work.current.size);
    return key;
  }

  function completeWork(key: string) {
    work.current.delete(key);
    setWorkCount(work.current.size);
  }

  function choose(files: "keep" | "revert") {
    if (request.current || !frozen.current) return;
    setPreview(null);
    setError(null);
    setPending(true);
    const key = id();
    request.current = { id: key, kind: "send" };
    const { draft, attachments } = frozen.current;
    post("rewindAndResend", {
      sessionId,
      messageId: message.id,
      files,
      text: draft.text,
      segments: draft.segments,
      attachments,
    }, key);
  }

  function submit() {
    if (pending || work.current.size || attachments.some((a) => a.unavailable)) return;
    const draft = editor.current?.getDraft();
    if (!draft || !draft.hasContent) return;
    frozen.current = { draft, attachments: [...attachments], busy };
    if (!hasWrites) {
      choose("keep");
      return;
    }
    const key = id();
    request.current = { id: key, kind: "preview" };
    setError(null);
    setPending(true);
    post("previewRewind", { sessionId, messageId: message.id }, key);
  }

  useEffect(() => {
    mounted.current = true;
    const onMessage = (event: MessageEvent) => {
      const frame = event.data;
      if (frame?.channel !== "event" || !frame.content || frame.content.sessionId !== sessionId) return;
      const content = frame.content;
      if (content.type === "editContextResult" && work.current.has(content.operationId)) {
        setAttachments((old) => [...old, ...content.attachments]);
        editor.current?.insertReferences(content.references as WebviewReference[]);
        if (content.error) setError(content.error);
        return;
      }
      if (content.type === "composerWorkResult" && work.current.has(content.operationId)) {
        completeWork(content.operationId);
        if (content.error) setError(content.error);
        return;
      }
      if (content.type === "contextSearchResult" && content.requestId === searchRequest.current) {
        setSearch({
          query: content.query,
          matches: content.matches,
          loading: false,
          truncated: content.truncated,
        });
        return;
      }
      const pendingRequest = request.current;
      if (!pendingRequest || content.requestId !== pendingRequest.id) return;
      const kind = pendingRequest.kind;
      request.current = null;
      setPending(false);
      if (kind === "preview" && content.success && isPreviewRewindResponse(content.preview)) {
        setPreview(content.preview);
        return;
      }
      if (kind === "send" && (content.success || content.error === "rewind_target_stale")) {
        closeRef.current();
        return;
      }
      setError(content.errorDetail ?? content.error ?? "未能重新发送，修改稿已保留。");
      if (content.error === "revert_unavailable") {
        const key = id();
        request.current = { id: key, kind: "preview" };
        setPending(true);
        post("previewRewind", { sessionId, messageId: message.id }, key);
      }
    };
    const outside = (event: PointerEvent) => {
      const wasInside = inside.current;
      inside.current = false;
      if (wasInside || protectedRef.current || event.button !== 0) return;
      const target = event.target as HTMLElement;
      // A scroll-bar press is scrolling, not cancellation.
      if (target instanceof HTMLElement && target.scrollHeight > target.clientHeight) {
        const rect = target.getBoundingClientRect();
        if (event.clientX >= rect.left + target.clientWidth) return;
      }
      closeRef.current();
    };
    const resetPointerOwner = () => { inside.current = false; };
    document.addEventListener("pointerdown", resetPointerOwner, true);
    window.addEventListener("message", onMessage);
    document.addEventListener("pointerdown", outside);
    const focus = window.setTimeout(() =>
      root.current?.querySelector<HTMLElement>('[contenteditable="true"]')?.focus(), 0);
    return () => {
      mounted.current = false;
      window.clearTimeout(focus);
      if (searchTimer.current) clearTimeout(searchTimer.current);
      window.removeEventListener("message", onMessage);
      document.removeEventListener("pointerdown", outside);
      document.removeEventListener("pointerdown", resetPointerOwner, true);
    };
  }, [sessionId, message.id]);

  useEffect(() => {
    const positionMenus = () => {
      const element = root.current;
      if (!element) return;
      const rect = element.getBoundingClientRect();
      const below = window.innerHeight - rect.bottom >= rect.top;
      element.style.setProperty("--inline-menu-left", `${Math.max(8, rect.left)}px`);
      element.style.setProperty("--inline-menu-top", below ? `${rect.bottom + 4}px` : "auto");
      element.style.setProperty("--inline-menu-bottom", below ? "auto" : `${window.innerHeight - rect.top + 4}px`);
      element.style.setProperty("--inline-menu-width", `${Math.max(160, Math.min(rect.width, window.innerWidth - rect.left - 8))}px`);
      element.style.setProperty("--inline-menu-height", `${Math.max(100, (below ? window.innerHeight - rect.bottom : rect.top) - 16)}px`);
    };
    positionMenus();
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(positionMenus);
    if (root.current) observer?.observe(root.current);
    window.addEventListener("resize", positionMenus);
    document.addEventListener("scroll", positionMenus, true);
    return () => {
      observer?.disconnect();
      window.removeEventListener("resize", positionMenus);
      document.removeEventListener("scroll", positionMenus, true);
    };
  }, []);

  const disabled = pending || !!preview;
  return (
    <div
      ref={root}
      className="tc-inline-editor"
      data-message-kind="user"
      data-message-id={message.id}
      onPointerDownCapture={() => { inside.current = true; }}
      data-testid="inline-message-editor"
    >
      <ComposerSurface
        {...composerProps}
        ref={editor}
        instanceId="edit"
        initialDraft={initialDraft}
        busy={false}
        canInterrupt={false}
        commandPending={pending}
        canPrompt={!disabled && !composerProps.commandPending}
        onCancelEdit={() => { if (!protectedRef.current) onClose(); }}
        onSubmit={submit}
        onDraftChange={() => undefined}
        attachments={attachments}
        feedback={error ? { hasErrors: true, message: error } : null}
        onRemoveAttachment={(key) => setAttachments((old) => old.filter((a) => a.id !== key))}
        onOpenAttachment={(a) => {
          if (a.kind === "image" && a.fullUri) setZoomedImage({ alt: a.filename, src: a.fullUri });
          else composerProps.onOpenAttachment(a);
        }}
        onPickContext={() => {
          const operationId = beginWork();
          post("pickContext", { sessionId, target: "edit", operationId });
        }}
        onResolveDrop={(uris) => {
          const operationId = beginWork();
          post("resolveDrop", { sessionId, target: "edit", operationId, uris });
        }}
        onPrepareAttachments={(preparation) => {
          const operationId = beginWork();
          void preparation
            .then((files) => {
              if (mounted.current) post("attachFiles", { sessionId, target: "edit", operationId, files });
            })
            .catch((e) => {
              if (mounted.current) {
                completeWork(operationId);
                setError(String(e));
              }
            });
        }}
        contextSearchLoading={search.loading}
        contextSearchMatches={search.matches}
        contextSearchQuery={search.query}
        contextSearchTruncated={search.truncated}
        onContextSearchOpen={() => undefined}
        onContextSearchClose={() => { searchRequest.current = null; }}
        onContextSearchQueryChange={(query) => {
          if (searchTimer.current) clearTimeout(searchTimer.current);
          setSearch((s) => ({ ...s, query, loading: true }));
          const requestId = id();
          searchRequest.current = requestId;
          searchTimer.current = setTimeout(() =>
            post("searchContext", { sessionId, query, requestId }), 250);
        }}
      />
      <ImageLightbox image={zoomedImage} onClose={() => setZoomedImage(null)} />
      {workCount > 0 ? <div role="status" className="tc-inline-editor__status">正在准备附件…</div> : null}
      {pending ? (
        <div role="status" className="tc-inline-editor__status">
          {request.current?.kind === "preview" ? "正在读取恢复信息…"
            : frozen.current?.busy ? "正在停止当前任务…" : "正在重发…"}
        </div>
      ) : null}
      {preview ? (
        <EditConfirmDialog
          preview={preview}
          busy={frozen.current?.busy ?? busy}
          onCancel={() => { setPreview(null); frozen.current = null; }}
          onChoose={choose}
        />
      ) : null}
    </div>
  );
}
