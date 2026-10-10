/**
 * ImagePreviewPanel — standalone VS Code WebviewPanel for image preview.
 *
 * Covers all M3 sub-items:
 * - Central object-fit:contain canvas (stage)
 * - "Attached image n / total" title
 * - Bottom filmstrip with horizontal scroll
 * - Arrow / Home / End navigation
 * - Close / Escape
 * - Copy via ClipboardItem
 * - Save via Host (showSaveDialog + workspace.fs)
 * - a11y: landmarks, focus management, aria-live, reduced-motion, high contrast
 */

import { useEffect, useRef, useState, useCallback } from "react";
import { useT } from "../i18n/LocaleProvider";
import { t as defaultT } from "../../../src/shared/i18n";
import { COPY_FLASH_MS } from "../components/copyFeedback";
import type {
  ImagePreviewDomAction,
  PreviewPicture,
  PreviewSection,
} from "../../../src/shared/imagePreviewProtocol";

// ── vscode API bridge ──

type PreviewVsCodeApi = {
  postMessage(message: Record<string, unknown>): void;
  getState(): Record<string, unknown> | undefined;
  setState(state: Record<string, unknown>): void;
};

declare function acquireVsCodeApi(): PreviewVsCodeApi;
let vscodeApi: PreviewVsCodeApi =
  typeof acquireVsCodeApi === "function"
    ? acquireVsCodeApi()
    : {
        getState: () => undefined,
        postMessage: () => undefined,
        setState: () => undefined,
      };

export function setPreviewVsCodeApiForTests(api: PreviewVsCodeApi): void {
  vscodeApi = api;
}

// ── State ──

interface PreviewState {
  sections: PreviewSection[];
  activeId: string;
  displayLabel: string;
  position: number;
  total: number;
}

export function collectPictures(sections: PreviewSection[]): PreviewPicture[] {
  const result: PreviewPicture[] = [];
  for (const section of sections) {
    for (const pic of section.pictures) {
      result.push(pic);
    }
  }
  return result;
}

export async function convertImageBlobToPng(blob: Blob): Promise<Blob> {
  if (blob.type === "image/png") return blob;
  const objectUrl = URL.createObjectURL(blob);
  try {
    const image = new Image();
    image.decoding = "async";
    const loaded = new Promise<void>((resolve, reject) => {
      image.onload = () => resolve();
      image.onerror = () => reject(new Error(defaultT("preview.decodeFailed")));
    });
    image.src = objectUrl;
    await loaded;
    const canvas = document.createElement("canvas");
    canvas.width = image.naturalWidth;
    canvas.height = image.naturalHeight;
    const context = canvas.getContext("2d");
    if (!context || canvas.width === 0 || canvas.height === 0) {
      throw new Error(defaultT("preview.canvasFailed"));
    }
    context.drawImage(image, 0, 0);
    return await new Promise<Blob>((resolve, reject) => {
      canvas.toBlob(
        (converted) =>
          converted
            ? resolve(converted)
            : reject(new Error(defaultT("preview.convertFailed"))),
        "image/png",
      );
    });
  } finally {
    URL.revokeObjectURL(objectUrl);
  }
}

/**
 * Read a picture's bytes back with the media type the resource protocol could not supply.
 *
 * VS Code types a webview resource from its file extension, and blobs are named by content
 * hash, so everything comes back as an unknown type. Rasters survive that — decoders sniff
 * the bytes — but SVG does not, and neither does the clipboard, both of which go by the
 * declared type. The attachment's real mime type is known here, so restate it.
 */
async function typedBlobForPicture(picture: PreviewPicture): Promise<Blob> {
  const response = await fetch(picture.fullUri);
  if (!response.ok) {
    throw new Error(defaultT("preview.readFailed"));
  }
  return new Blob([await response.arrayBuffer()], { type: picture.mimeType });
}

export async function clipboardBlobForPicture(
  picture: PreviewPicture,
): Promise<Blob> {
  const blob = await typedBlobForPicture(picture);
  const clipboardItem = globalThis.ClipboardItem;
  const supportsOriginal =
    blob.type === "image/png" ||
    (typeof clipboardItem?.supports === "function" &&
      clipboardItem.supports(blob.type));
  return supportsOriginal ? blob : convertImageBlobToPng(blob);
}

// ── Main Component ──

export function PreviewPanel() {
  const t = useT();
  const tRef = useRef(t);
  tRef.current = t;
  const [state, setState] = useState<PreviewState | null>(null);
  const [zoom, setZoom] = useState<"fit" | number>("fit");
  const [copyState, setCopyState] = useState<"idle" | "copied">("idle");
  const stageRef = useRef<HTMLDivElement>(null);
  const filmstripRef = useRef<HTMLDivElement>(null);
  const activeThumbRef = useRef<HTMLButtonElement>(null);
  const liveRegionRef = useRef<HTMLDivElement>(null);
  const copyResetTimerRef = useRef<number | null>(null);

  // Send ready message on mount and receive Host state updates.
  useEffect(() => {
    const handleMessage = (event: MessageEvent) => {
      const msg = event.data;
      if (msg.type === "preview.state" && msg.data) {
        setState(msg.data);
      }
      if (msg.type === "preview.saveResult" && msg.data) {
        announce(
          msg.data.cancelled
            ? tRef.current("preview.saveCancelled")
            : msg.data.success
              ? tRef.current("preview.saved")
              : tRef.current("preview.saveFailed", { detail: msg.data.error ?? tRef.current("error.unknown") }),
        );
      }
      if (msg.type === "preview.forceClose") {
        window.close();
      }
    };
    vscodeApi.postMessage({ type: "preview.ready", data: {} });
    window.addEventListener("message", handleMessage);
    stageRef.current?.focus();
    return () => window.removeEventListener("message", handleMessage);
  }, []);

  // Scroll active thumb into view
  useEffect(() => {
    if (state && activeThumbRef.current) {
      activeThumbRef.current.scrollIntoView({ behavior: "smooth", block: "nearest", inline: "center" });
    }
  }, [state?.activeId]);

  // Announce position changes
  useEffect(() => {
    if (state) {
      announce(t("preview.position", { label: state.displayLabel, position: state.position, total: state.total }));
    }
  }, [state?.activeId, t]);

  const announce = useCallback((text: string) => {
    if (liveRegionRef.current) {
      liveRegionRef.current.textContent = text;
    }
  }, []);

  const clearCopyResetTimer = useCallback(() => {
    if (copyResetTimerRef.current !== null) {
      window.clearTimeout(copyResetTimerRef.current);
      copyResetTimerRef.current = null;
    }
  }, []);

  useEffect(() => clearCopyResetTimer, [clearCopyResetTimer]);

  const pictures = state ? collectPictures(state.sections) : [];
  const activeIndex = state ? pictures.findIndex((p) => p.id === state.activeId) : -1;
  const activePicture = activeIndex >= 0 ? pictures[activeIndex] : null;

  /**
   * Full-resolution images live in the DOM only for the current picture and its two
   * neighbours.
   *
   * Paging is the reason the neighbours are included: pre-decoding them is what makes
   * an arrow keypress feel instant. Going wider than that buys nothing — with eleven
   * 4000x3000 photos, every extra slot is another 48MB of bitmap for an image nobody
   * has asked to see.
   */
  const preloadedIds = new Set(
    activeIndex < 0
      ? []
      : [activeIndex - 1, activeIndex, activeIndex + 1]
          .filter((index) => index >= 0 && index < pictures.length)
          .map((index) => pictures[index]!.id),
  );

  /**
   * A source URL for the picture on screen that works for SVG too.
   *
   * Rasters go straight to the resource URL. SVG cannot: it is only rendered as an image
   * when the response says `image/svg+xml`, and a hash-named blob has no extension for
   * VS Code to derive that from. Re-fetching the bytes into a typed blob supplies the
   * type, and keeps the picture a vector instead of showing the rasterised copy that
   * exists for the model's benefit.
   */
  const [svgObjectUrl, setSvgObjectUrl] = useState<string | null>(null);
  const activeIsSvg = activePicture?.mimeType === "image/svg+xml";
  const activeFullUri = activePicture?.fullUri ?? null;
  useEffect(() => {
    if (!activeIsSvg || !activePicture) {
      setSvgObjectUrl(null);
      return;
    }
    let url: string | null = null;
    let cancelled = false;
    void (async () => {
      try {
        const blob = await typedBlobForPicture(activePicture);
        if (cancelled) return;
        url = URL.createObjectURL(blob);
        setSvgObjectUrl(url);
      } catch {
        // Falls through to the resource URL, which shows the browser's broken-image
        // state — the same outcome as before, and better than a blank stage.
        setSvgObjectUrl(null);
      }
    })();
    return () => {
      cancelled = true;
      if (url) URL.revokeObjectURL(url);
    };
  }, [activeIsSvg, activeFullUri]);
  const activeStageSrc = activeIsSvg
    ? (svgObjectUrl ?? activeFullUri)
    : activeFullUri;

  useEffect(() => {
    clearCopyResetTimer();
    setCopyState("idle");
  }, [activePicture?.id, clearCopyResetTimer]);

  // ── Navigation ──

  const goTo = useCallback((id: string) => {
    vscodeApi.postMessage({ type: "preview.select", data: { attachmentId: id } });
  }, []);

  const goPrev = useCallback(() => {
    if (activeIndex > 0) goTo(pictures[activeIndex - 1].id);
  }, [activeIndex, pictures, goTo]);

  const goNext = useCallback(() => {
    if (activeIndex < pictures.length - 1) goTo(pictures[activeIndex + 1].id);
  }, [activeIndex, pictures, goTo]);

  const goHome = useCallback(() => {
    if (pictures.length > 0) goTo(pictures[0].id);
  }, [pictures, goTo]);

  const goEnd = useCallback(() => {
    if (pictures.length > 0) goTo(pictures[pictures.length - 1].id);
  }, [pictures, goTo]);

  // ── Zoom ──

  const toggleZoom = useCallback(() => {
    setZoom((z) => (z === "fit" ? 1 : "fit"));
  }, []);

  const zoomIn = useCallback(() => {
    setZoom((z) => {
      if (typeof z !== "number") return 1.5;
      return Math.min(z + 0.5, 5);
    });
  }, []);

  const zoomOut = useCallback(() => {
    setZoom((z) => {
      if (typeof z !== "number") return 0.75;
      return Math.max(z - 0.5, 0.25);
    });
  }, []);

  // ── Keyboard ──

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        vscodeApi.postMessage({ type: "preview.close", data: {} });
        return;
      }
      if (e.key === "ArrowLeft") { goPrev(); e.preventDefault(); }
      if (e.key === "ArrowRight") { goNext(); e.preventDefault(); }
      if (e.key === "Home") { goHome(); e.preventDefault(); }
      if (e.key === "End") { goEnd(); e.preventDefault(); }
      if (e.key === "+" || e.key === "=") { zoomIn(); e.preventDefault(); }
      if (e.key === "-") { zoomOut(); e.preventDefault(); }
      if (e.key === "0") { setZoom("fit"); e.preventDefault(); }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [goPrev, goNext, goHome, goEnd, zoomIn, zoomOut]);

  // ── Copy (via ClipboardItem) ──

  const handleCopy = useCallback(async () => {
    if (!activePicture) return;
    try {
      if (typeof ClipboardItem === "undefined" || !navigator.clipboard?.write) {
        announce(t("preview.copyUnsupported"));
        return;
      }
      const blob = await clipboardBlobForPicture(activePicture);
      await navigator.clipboard.write([
        new ClipboardItem({ [blob.type]: blob }),
      ]);
      clearCopyResetTimer();
      setCopyState("copied");
      copyResetTimerRef.current = window.setTimeout(() => {
        setCopyState("idle");
        copyResetTimerRef.current = null;
      }, COPY_FLASH_MS);
      announce(t("preview.copied"));
    } catch {
      clearCopyResetTimer();
      setCopyState("idle");
      announce(t("common.copyFailed"));
    }
  }, [activePicture, announce, clearCopyResetTimer, t]);

  // Test-only bridge used by the Development Host visual acceptance harness.
  useEffect(() => {
    const handleTestMessage = (event: MessageEvent) => {
      const msg = event.data as {
        action?: ImagePreviewDomAction;
        requestId?: string;
        type?: string;
      };
      if (msg.type === "preview.__test.dom_action" && msg.action) {
        switch (msg.action.kind) {
          case "copy":
            void handleCopy();
            break;
          case "fit":
            setZoom("fit");
            break;
          case "next":
            goNext();
            break;
          case "previous":
            goPrev();
            break;
          case "zoomIn":
            zoomIn();
            break;
          case "zoomOut":
            zoomOut();
            break;
        }
        return;
      }
      if (msg.type !== "preview.__test.capture_dom" || !msg.requestId) {
        return;
      }
      vscodeApi.postMessage({
        type: "preview.__test.dom_snapshot",
        requestId: msg.requestId,
        snapshot: {
          activeId: state?.activeId ?? null,
          activeThumbIndex: activeIndex,
          copyButtonCopied: copyState === "copied",
          copyIconClass:
            document
              .querySelector<HTMLSpanElement>(
                'button[data-testid="preview-copy"] .codicon',
              )
              ?.className ?? null,
          downloadIconFontFamily:
            getComputedStyle(
              document.querySelector<HTMLSpanElement>('button[data-testid="preview-save"] .codicon')
                ?? document.body,
              "::before",
            ).fontFamily || null,
          position: state?.position ?? 0,
          stageClientWidth: stageRef.current?.clientWidth ?? 0,
          stageNaturalWidth:
            document.querySelector<HTMLImageElement>(
              '[data-testid="preview-stage-image"]',
            )?.naturalWidth ?? 0,
          stageScrollWidth: stageRef.current?.scrollWidth ?? 0,
          thumbCount: pictures.length,
          total: state?.total ?? 0,
          zoom,
        },
      });
    };
    window.addEventListener("message", handleTestMessage);
    return () => window.removeEventListener("message", handleTestMessage);
  }, [activeIndex, copyState, goNext, goPrev, handleCopy, pictures.length, state, zoom, zoomIn, zoomOut]);

  // ── Save (via Host) ──

  const handleSave = useCallback(() => {
    if (!activePicture) return;
    vscodeApi.postMessage({ type: "preview.save", data: { attachmentId: activePicture.id } });
  }, [activePicture]);

  // ── Close ──

  const handleClose = useCallback(() => {
    vscodeApi.postMessage({ type: "preview.close", data: {} });
  }, []);

  // ── Render ──

  if (!state || !activePicture) {
    return (
      <main role="main" style={{ display: "flex", alignItems: "center", justifyContent: "center", height: "100%" }}>
        <p>{t("preview.noImage")}</p>
      </main>
    );
  }

  const fitStyle: React.CSSProperties = zoom === "fit"
    ? { maxWidth: "100%", maxHeight: "100%", objectFit: "contain" }
    : { width: `${zoom * 100}%`, height: `${zoom * 100}%`, objectFit: "contain" };

  return (
    <main role="main" aria-label={t("image.preview")} style={{ display: "flex", flexDirection: "column", height: "100%" }}>
      {/* aria-live region for announcements */}
      <div ref={liveRegionRef} role="status" aria-live="polite" style={{ position: "absolute", width: 1, height: 1, overflow: "hidden", clip: "rect(0,0,0,0)" }} />

      {/* Top bar */}
      <header style={{ display: "flex", flexWrap: "wrap", alignItems: "center", gap: 6, padding: "8px 16px", borderBottom: "1px solid var(--vscode-panel-border, #333)", flex: "none" }}>
        <span title={state.displayLabel} style={{ fontWeight: 600, flex: 1, minWidth: 0, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{state.displayLabel}</span>
        <span style={{ opacity: 0.7, fontSize: 12, flex: "none", whiteSpace: "nowrap" }}>{state.position} / {state.total}</span>
        <ToolButton iconClass="codicon-zoom-out" onClick={zoomOut} label={t("preview.zoomOut")} shortcut="-" />
        <ToolButton
          iconClass={zoom === "fit" ? "codicon-screen-normal" : "codicon-screen-full"}
          onClick={toggleZoom}
          label={t(zoom === "fit" ? "preview.actualSize" : "preview.fit")}
          shortcut="0"
        />
        <ToolButton iconClass="codicon-zoom-in" onClick={zoomIn} label={t("preview.zoomIn")} shortcut="+" />
        <div style={{ width: 1, height: 24, background: "var(--vscode-panel-border, #333)" }} />
        <ToolButton
          className={copyState === "copied" ? "is-copied" : undefined}
          copied={copyState === "copied"}
          testId="preview-copy"
          iconClass={copyState === "copied" ? "codicon-check" : "codicon-copy"}
          onClick={handleCopy}
          label={t(copyState === "copied" ? "common.copied" : "preview.copy")}
          shortcut=""
        />
        <ToolButton iconClass="codicon-download" onClick={handleSave} label={t("preview.save")} testId="preview-save" shortcut="" />
        <div style={{ width: 1, height: 24, background: "var(--vscode-panel-border, #333)" }} />
        <ToolButton iconClass="codicon-close" onClick={handleClose} label={t("preview.close")} shortcut="Esc" />
      </header>

      {/* Stage */}
      <div
        ref={stageRef}
        data-testid="preview-stage"
        data-zoom={zoom}
        tabIndex={0}
        role="region"
        aria-label={t("preview.stage")}
        style={{ flex: 1, display: "flex", alignItems: "center", justifyContent: "center", overflow: "auto", padding: 16, background: "var(--vscode-editor-background, #1e1e1e)", cursor: zoom === "fit" ? "default" : "grab" }}
        onClick={toggleZoom}
      >
        <img
          src={activeStageSrc ?? undefined}
          alt={activePicture.filename}
          data-testid="preview-stage-image"
          style={fitStyle}
          draggable={false}
        />
        {/*
          The neighbours, decoded but not shown, so paging does not stall on a decode.
          Kept inside the stage so they share its lifetime.
        */}
        {pictures
          .filter((pic) => pic.id !== activePicture.id && preloadedIds.has(pic.id))
          .map((pic) => (
            <img
              key={pic.id}
              src={pic.fullUri}
              alt=""
              aria-hidden="true"
              data-testid="preview-neighbour-image"
              decoding="async"
              style={{ height: 0, position: "absolute", width: 0 }}
            />
          ))}
      </div>

      {/* Filmstrip */}
      <nav aria-label={t("preview.thumbnails")} style={{ flex: "none", borderTop: "1px solid var(--vscode-panel-border, #333)", padding: "8px 0" }}>
        <div style={{ display: "flex", alignItems: "center", gap: 4, padding: "0 12px" }}>
          <FilmstripButton iconClass="codicon-chevron-left" onClick={goPrev} disabled={activeIndex <= 0} label={t("preview.previous")} />
          <div
            ref={filmstripRef}
            role="list"
            style={{ display: "flex", flexWrap: "nowrap", overflowX: "auto", gap: 6, scrollbarWidth: "thin", padding: "2px 4px", flex: 1 }}
          >
            {pictures.map((pic, idx) => (
              <div
                key={pic.id}
                role="listitem"
                style={{ flex: "none" }}
              >
                <button
                  ref={idx === activeIndex ? activeThumbRef : undefined}
                  aria-label={t("preview.thumbnail", { filename: pic.filename, position: idx + 1, total: pictures.length })}
                  aria-current={idx === activeIndex ? "true" : undefined}
                  className={idx === activeIndex ? "ip-thumb ip-thumb--active" : "ip-thumb"}
                  style={{
                    width: 48, height: 48, borderRadius: 4, overflow: "hidden", cursor: "pointer",
                    border: idx === activeIndex ? "2px solid var(--vscode-focusBorder)" : "2px solid transparent",
                    padding: 0, background: "none",
                  }}
                  onClick={() => goTo(pic.id)}
                  type="button"
                >
                  {pic.thumbUri ? (
                    <img
                      src={pic.thumbUri}
                      alt=""
                      data-testid="preview-filmstrip-image"
                      decoding="async"
                      style={{ width: "100%", height: "100%", objectFit: "cover", display: "block" }}
                      loading="lazy"
                    />
                  ) : (
                    // No thumbnail yet. Showing the original here would mean a
                    // full-resolution decode per filmstrip entry, which is the cost this
                    // panel is built to avoid.
                    <span
                      data-testid="preview-filmstrip-placeholder"
                      style={{
                        background: "var(--vscode-editorWidget-background, #252526)",
                        display: "block",
                        height: "100%",
                        width: "100%",
                      }}
                    />
                  )}
                </button>
              </div>
            ))}
          </div>
          <FilmstripButton iconClass="codicon-chevron-right" onClick={goNext} disabled={activeIndex >= pictures.length - 1} label={t("preview.next")} />
        </div>
      </nav>
    </main>
  );
}

function ToolButton({
  testId,
  className,
  copied = false,
  iconClass,
  onClick,
  label,
  shortcut,
}: {
  className?: string;
  copied?: boolean;
  iconClass: string;
  onClick(): void;
  label: string;
  shortcut: string;
  testId?: string;
}) {
  return (
    <button
      aria-label={label}
      data-testid={testId}
      className={className}
      title={`${label}${shortcut ? ` (${shortcut})` : ""}`}
      onClick={onClick}
      style={{
        background: "none",
        border: "1px solid transparent",
        borderRadius: 4,
        color: copied
          ? "var(--vscode-testing-iconPassed, var(--vscode-terminal-ansiGreen, #73c991))"
          : "var(--vscode-foreground, #ccc)",
        cursor: "pointer",
        padding: "4px 8px",
        flex: "none",
        fontSize: 0,
        lineHeight: 1,
        display: "inline-flex",
        alignItems: "center",
        gap: 4,
      }}
      onMouseEnter={(e) => { e.currentTarget.style.borderColor = "var(--vscode-panel-border, #555)"; }}
      onMouseLeave={(e) => { e.currentTarget.style.borderColor = "transparent"; }}
    >
      <span
        aria-hidden="true"
        className={`codicon ${iconClass}`}
        style={{ fontSize: 16, lineHeight: 1 }}
      />
    </button>
  );
}

function FilmstripButton({
  iconClass,
  onClick,
  disabled,
  label,
}: {
  iconClass: string;
  onClick(): void;
  disabled: boolean;
  label: string;
}) {
  return (
    <button
      aria-label={label}
      disabled={disabled}
      onClick={onClick}
      style={{
        background: "none", border: "1px solid var(--vscode-panel-border, #333)", borderRadius: 4,
        color: disabled ? "var(--vscode-disabledForeground, #555)" : "var(--vscode-foreground, #ccc)",
        cursor: disabled ? "default" : "pointer", padding: "4px 10px", fontSize: 0, lineHeight: 1, flex: "none",
      }}
    >
      <span
        aria-hidden="true"
        className={`codicon ${iconClass}`}
        style={{ fontSize: 16, lineHeight: 1 }}
      />
    </button>
  );
}
