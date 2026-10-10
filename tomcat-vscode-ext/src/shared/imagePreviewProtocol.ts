/**
 * Shared protocol types for the ImagePreview WebviewPanel.
 *
 * Architecture:
 *   Chat Webview          Extension Host              Preview Webview
 *      │ openImagePreview      │                            │
 *      ├────Intent────────────>│                            │
 *      │                       │── createWebviewPanel ──────>│
 *      │                       │      (or reveal existing)   │
 *      │                       │   preview.ready             │
 *      │                       │<─────{sessionId}────────────│
 *      │                       │   preview.state             │
 *      │                       │<─────{sections,activeId}───>│
 *      │                       │   preview.select            │
 *      │                       │<─────{attachmentId}─────────│
 *      │                       │   preview.save             │
 *      │                       │<─────{attachmentId}─────────│
 *      │                       │   preview.close             │
 *      │                       │<─────{ }────────────────────│
 */

/**
 * A single picture in the preview panel — two URLs, no bytes.
 *
 * The split matters here more than anywhere else in the UI. The filmstrip shows every
 * image at thumbnail size while the stage shows one at full size. Pointing both at the
 * same source means opening a preview over eleven photos decodes eleven full-resolution
 * bitmaps to draw eleven 64px squares.
 */
export interface PreviewPicture {
  filename: string;
  /** Full-resolution source. Loaded only for the picture on screen and its neighbours. */
  fullUri: string;
  /** Stable identifier: `${messageId}:image:${partIndex}` for history, UUID for a draft. */
  id: string;
  mimeType: string;
  /**
   * Downsampled source for the filmstrip, or null until one has been generated.
   *
   * Never falls back to {@link fullUri}: a filmstrip of eleven 48px squares backed by
   * 4000x3000 originals is half a gigabyte of decoded bitmaps. The strip shows a
   * placeholder instead, and the chat webview fills the thumbnail in.
   */
  thumbUri: string | null;
}

/** One preview section — a user turn (history) or the current pending draft. */
export interface PreviewSection {
  label: string;
  /** Product-owned section kind; `label` remains the unmodified tool title or turn number. */
  kind?: "pending" | "sent" | "tool";
  pictures: PreviewPicture[];
}

/** Webview → Host: ready signal from the Panel Webview. */
export interface PreviewReady {
  type: "preview.ready";
  data: {
    sessionId?: string;
  };
}

/** Host → Preview Webview: full state push. */
export interface PreviewState {
  type: "preview.state";
  data: {
    locale?: import("./i18n").Locale;
    /** Sections grouped by user turn + optional pending section at end. */
    sections: PreviewSection[];
    /** Active (currently displayed) picture id. */
    activeId: string;
    /** Display label, e.g. "Attached image 2" */
    displayLabel: string;
    /** 1-based position within collection. */
    position: number;
    /** Total pictures across all sections. */
    total: number;
  };
}

/** Preview Webview → Host: user selected a different picture. */
export interface PreviewSelect {
  type: "preview.select";
  data: {
    attachmentId: string;
  };
}

/** Preview Webview → Host: user wants to save as… */
export interface PreviewSave {
  type: "preview.save";
  data: {
    attachmentId: string;
  };
}

/** Preview Webview → Host: user closed the panel or hit Escape. */
export interface PreviewClose {
  type: "preview.close";
  data: Record<string, never>;
}

/** Host → Preview Webview: save-as result. */
export interface PreviewSaveResult {
  type: "preview.saveResult";
  data: {
    success: boolean;
    cancelled?: boolean;
    error?: string | null;
    /** Path the user chose (only on success). */
    savedPath?: string | null;
  };
}

/** Host → Preview Webview: close the panel (e.g. session deleted). */
export interface PreviewForceClose {
  type: "preview.forceClose";
  data: Record<string, never>;
}

/** Test-only action used by the real VS Code acceptance harness. */
export interface ImagePreviewDomAction {
  kind: "copy" | "fit" | "next" | "previous" | "zoomIn" | "zoomOut";
}

/** Minimal visual state returned by the preview Webview test bridge. */
export interface ImagePreviewDomSnapshot {
  activeId: string | null;
  activeThumbIndex: number;
  copyButtonCopied?: boolean;
  copyIconClass?: string | null;
  downloadIconFontFamily?: string | null;
  position: number;
  stageClientWidth: number;
  /**
   * Intrinsic width of the picture on the stage, or 0 if it did not decode.
   *
   * The one way to tell a rendered image from a broken one without looking at a
   * screenshot. It matters for SVG in particular, which reaches the stage through a
   * different route than rasters do and can fail while everything around it looks fine.
   */
  stageNaturalWidth: number;
  stageScrollWidth: number;
  thumbCount: number;
  total: number;
  zoom: "fit" | number;
}
