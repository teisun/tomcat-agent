import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { freshOccurrences } from "../../../src/shared/composerOccurrences";
import type { WebviewMessageQueue, WebviewQueueItem } from "../../../src/ui/webview/protocol";
import type { ContextSearchMatch, VsCodeApiLike, WebviewIntent, WebviewPendingAttachment } from "../types";
import { DockSection } from "./DockSection";
import { ComposerSurface, type ComposerSurfaceProps } from "./ComposerSurface";
import type { ComposerHandle } from "./Composer";
import { AttachmentStrip } from "./AttachmentStrip";
import { ImageLightbox, type ZoomedImage } from "./ImageLightbox";

export function QueuedMessageEditor({ item, sessionId, composerProps, vscodeApi, action }: {
  item: WebviewQueueItem; sessionId: string; composerProps: ComposerSurfaceProps; vscodeApi: VsCodeApiLike;
  action(action: "save" | "cancel", data?: Partial<Extract<WebviewIntent, { type: "queueAction" }>["data"]>): void;
}) {
  const editor = useRef<ComposerHandle>(null);
  const [initialDraft] = useState(() => ({ text: item.text, hasContent: true,
    segments: freshOccurrences(item.segments.length ? item.segments : [{ type: "text", text: item.text }]),
  }));
  const [attachments, setAttachments] = useState<WebviewPendingAttachment[]>(item.attachments);
  const [error, setError] = useState<string | null>(null);
  const work = useRef(new Set<string>());
  const [working, setWorking] = useState(false);
  const [search, setSearch] = useState({ query: "", matches: [] as ContextSearchMatch[], loading: false, truncated: false });
  const searchId = useRef("");
  const [zoom, setZoom] = useState<ZoomedImage | null>(null);
  function begin() { const id = crypto.randomUUID(); work.current.add(id); setWorking(true); return id; }
  function post(type: WebviewIntent["type"], data: object) { vscodeApi.postMessage({ type, messageId: crypto.randomUUID(), data } as WebviewIntent); }
  useEffect(() => {
    function receive(event: MessageEvent) {
      const c = event.data?.channel === "event" ? event.data.content : null;
      if (!c || c.sessionId !== sessionId) return;
      if (c.type === "editContextResult" && work.current.has(c.operationId)) {
        setAttachments(old => [...old, ...c.attachments]); editor.current?.insertReferences(c.references);
        if (c.error) setError(c.error);
      }
      if (c.type === "composerWorkResult" && work.current.has(c.operationId)) {
        work.current.delete(c.operationId); setWorking(work.current.size > 0); if (c.error) setError(c.error);
      }
      if (c.type === "contextSearchResult" && c.requestId === searchId.current) setSearch({ query: c.query, matches: c.matches, loading: false, truncated: c.truncated });
    }
    window.addEventListener("message", receive);
    return () => window.removeEventListener("message", receive);
  }, [sessionId]);
  useEffect(() => { document.querySelector<HTMLElement>('[data-testid="queue-edit-composer-input"]')?.focus(); }, []);
  function save() {
    const draft = editor.current?.getDraft();
    if (!draft?.hasContent || work.current.size || attachments.some(a => a.unavailable)) return;
    action("save", { text: draft.text, segments: draft.segments, attachments: attachments.map(a => ({
      id: a.id, kind: a.kind, filename: a.filename, mimeType: a.mimeType, blobSha: a.blobSha,
      providerSha: a.providerSha, bytes: a.bytes ?? 0, hasThumb: a.hasThumb,
    })) });
  }
  return <section className="tc-queue-editor" data-testid="queue-editor" aria-label="Editing queued message">
    <ComposerSurface {...composerProps} ref={editor} initialDraft={initialDraft} instanceId="queue-edit"
      header={<><span>Editing queued message</span><button type="button" className="tc-composer__cancel" data-testid="queue-edit-cancel" onClick={() => action("cancel")}>Cancel</button></>}
      canChangeConfig={composerProps.canChangeConfig ?? !composerProps.busy}
      hideDragHint submitAriaLabel="Save queued message" busy={false} canInterrupt={false} canPrompt={composerProps.canPrompt && !composerProps.commandPending}
      submitDisabled={working || attachments.some(a => a.unavailable)}
      attachmentsPending={working}
      onCancelEdit={() => action("cancel")} onSubmit={save} onDraftChange={() => undefined} attachments={attachments}
      feedback={error ? { hasErrors: true, message: error } : null}
      onRemoveAttachment={id => setAttachments(old => old.filter(a => a.id !== id))}
      onOpenAttachment={a => { if (a.kind === "image" && a.fullUri) setZoom({ alt: a.filename, src: a.fullUri, mimeType: a.mimeType }); else composerProps.onOpenAttachment(a); }}
      onPickContext={() => post("pickContext", { sessionId, target: "edit", operationId: begin() })}
      onResolveDrop={uris => post("resolveDrop", { sessionId, target: "edit", operationId: begin(), uris })}
      onPrepareAttachments={promise => {
        const operationId = begin();
        void promise.then(files => post("attachFiles", { sessionId, target: "edit", operationId, files })).catch(error => {
          work.current.delete(operationId); setWorking(work.current.size > 0); setError(String(error));
        });
      }}
      contextSearchLoading={search.loading} contextSearchMatches={search.matches} contextSearchQuery={search.query} contextSearchTruncated={search.truncated}
      onContextSearchClose={() => setSearch(old => ({ ...old, loading: false, matches: [] }))}
      onContextSearchOpen={() => undefined}
      onContextSearchQueryChange={query => {
        searchId.current = crypto.randomUUID(); setSearch(old => ({ ...old, query, loading: true }));
        post("searchContext", { sessionId, kind: "file", query, requestId: searchId.current });
      }} />
    <ImageLightbox image={zoom} onClose={() => setZoom(null)} />
  </section>;
}

export function MessageQueueDock({ sessionId, queue, busy, composerProps, vscodeApi }: {
  sessionId: string; queue: WebviewMessageQueue; busy: boolean; composerProps: ComposerSurfaceProps; vscodeApi: VsCodeApiLike;
}) {
  const root = useRef<HTMLDivElement>(null);
  const focusedRow = useRef<string | null>(null);
  const [zoom, setZoom] = useState<ZoomedImage | null>(null);
  const previousEditor = useRef(queue.editingId);
  useLayoutEffect(() => () => {
    if (root.current?.contains(document.activeElement)) queueMicrotask(() => {
      if (document.activeElement === document.body) document.querySelector<HTMLElement>('[data-testid="composer-input"]')?.focus();
    });
  }, []);
  useEffect(() => {
    const editing = previousEditor.current;
    previousEditor.current = queue.editingId;
    if (document.activeElement !== document.body) return;
    if (editing && !queue.editingId) {
      const row = [...(root.current?.querySelectorAll<HTMLElement>('[data-user-message-id]') ?? [])].find(node => node.dataset.userMessageId === editing);
      row?.querySelector<HTMLButtonElement>("button")?.focus();
    } else if (focusedRow.current && !queue.items.some(item => item.userMessageId === focusedRow.current)) {
      root.current?.querySelector<HTMLButtonElement>("button")?.focus(); focusedRow.current = null;
    }
  }, [queue.items, queue.editingId]);
  const count = queue.items.filter(item => item.status === "queued").length;
  function action(item: WebviewQueueItem, action: "send" | "edit" | "cancel" | "delete" | "save", data?: object) {
    vscodeApi.postMessage({ type: "queueAction", messageId: crypto.randomUUID(), data: { sessionId, userMessageId: item.userMessageId, action, ...data } } as WebviewIntent);
  }
  return <DockSection label="messages" title={`${count ? `${count} Queued` : "Messages"}${queue.paused ? " · 已暂停" : ""}`} defaultExpanded testId="messages-dock">
    <div ref={root} className="tc-message-queue" role="list" aria-label="待发消息">
      {queue.items.map(item => <div key={item.userMessageId} className={`tc-message-queue__row${queue.editingId === item.userMessageId ? " tc-message-queue__row--editing" : ""}`} aria-current={queue.editingId === item.userMessageId ? true : undefined} role="listitem" data-testid="queue-row" data-user-message-id={item.userMessageId}
        onFocusCapture={() => { focusedRow.current = item.userMessageId; }} onBlurCapture={event => { if (event.relatedTarget && !event.currentTarget.contains(event.relatedTarget as Node)) focusedRow.current = null; }}>
          <span className="tc-message-queue__status" aria-label={item.status === "queued" ? "待发" : "正在发送"}>{item.status === "queued" ? "○" : <span aria-hidden="true" className="codicon codicon-loading tc-codicon-spin" />}</span>
          <div className="tc-message-queue__body"><div>{item.text || item.segments.map(s => s.type === "text" ? s.text : s.label).join("") || "附件"}</div>
            {item.attachments.length ? <AttachmentStrip attachments={item.attachments} readonly onOpen={a => { if (a.kind === "image" && a.fullUri) setZoom({ alt: a.filename, src: a.fullUri, mimeType: a.mimeType }); else composerProps.onOpenAttachment(a); }} /> : null}
          </div>
          {item.status === "queued" ? <div className="tc-message-queue__actions">
            {queue.editingId !== item.userMessageId ? <>
            <button type="button" data-testid="queue-edit" aria-label="编辑待发消息" title="编辑待发消息" onClick={() => action(item, "edit")}><span aria-hidden="true" className="codicon codicon-edit" /></button>
            <button type="button" data-testid="queue-send" aria-label={busy ? "立即插入当前任务，沿用当前模式和模型" : "立即发送"} title={busy ? "立即插入当前任务，沿用当前模式和模型" : "立即发送"} onClick={() => action(item, "send")}>↑</button>
            </> : null}
            <button type="button" data-testid="queue-delete" aria-label="删除待发消息" title="删除待发消息" onClick={() => action(item, "delete")}><span aria-hidden="true" className="codicon codicon-trash" /></button>
          </div> : null}
      </div>)}
    </div>
    <ImageLightbox image={zoom} onClose={() => setZoom(null)} />
  </DockSection>;
}
