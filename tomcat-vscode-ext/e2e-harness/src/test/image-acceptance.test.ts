import * as assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import * as zlib from "node:zlib";

import * as vscode from "vscode";

type DomSnapshot = {
  activeSessionId: string | null;
  attachmentFetchProbe: string | null;
  /** Decoded bitmap cost of the strip, measured from what Chromium actually decoded. */
  attachmentBitmapBytes: number;
  attachmentBitmaps: Array<{
    height: number;
    resolution: string | null;
    width: number;
  }>;
  blockedInlineImageTexts: string[];
  attachmentSkeletonCount: number;
  attachmentUnavailableCount: number;
  composerText: string | null;
  focusedTestId: string | null;
  fullResolutionProbe: string | null;
  historyAttachmentThumbCount: number;
  historyPdfChipTitles: string[];
  historyAttachmentUnavailableCount: number;
  html: string;
  imagePipelineProbe: string | null;
  inlineImages: Array<{
    cursor: string;
    naturalHeight: number;
    naturalWidth: number;
    src: string;
  }>;
  lightboxImageNaturalWidth: number;
  lightboxImageSrc: string | null;
  lightboxVisible: boolean;
  messageTexts: string[];
  pendingAttachmentStripClientWidth: number;
  pendingAttachmentStripOverflowing: boolean;
  pendingAttachmentStripScrollWidth: number;
  /** Entries in the draft strip, thumbnail or placeholder alike. */
  pendingAttachmentItemCount: number;
  pendingPdfChipTitles: string[];
  pendingAttachmentThumbCount: number;
  timelineKinds: string[];
};

/** One entry of the in-webview image pipeline probe. */
type PipelineProbeResult = {
  error?: string;
  label: string;
  providerIsPng?: boolean;
  providerSize?: { height: number; width: number } | null;
  rasterised?: boolean;
  thumbSize?: { height: number; width: number } | null;
  thumbWithinBudget?: boolean;
  usedSourceFallback?: boolean;
  warnings?: string[];
};

type PreviewDomSnapshot = {
  activeId: string | null;
  activeThumbIndex: number;
  copyButtonCopied?: boolean;
  copyIconClass?: string | null;
  downloadIconFontFamily?: string | null;
  position: number;
  stageClientWidth: number;
  /** 0 when the picture on the stage failed to decode. */
  stageNaturalWidth: number;
  stageScrollWidth: number;
  thumbCount: number;
  total: number;
  zoom: "fit" | number;
};

type TomcatExtensionApi = {
  __testing: {
    captureImagePreviewDom(): Promise<PreviewDomSnapshot>;
    captureWebviewDom(): Promise<DomSnapshot>;
    clearObservedEvents(): void;
    getAttachmentDiagnostics(): {
      attachmentRoot: string | null;
      attachmentRootResolved: boolean;
      blobsDirExists: boolean;
      resourceRoots: string[];
      thumbsDirExists: boolean;
    };
    dispatchImagePreviewDomAction(action: {
      kind: "copy" | "fit" | "next" | "previous" | "zoomIn" | "zoomOut";
    }): Promise<void>;
    executeCommand(command: string, ...args: unknown[]): Thenable<unknown>;
    focusWebview(): Promise<void>;
    getWebviewState(): {
      activeSessionId: string | null;
      sessionViews: Record<string, unknown>;
    };
    reloadWebview(): Promise<void>;
    restartServe(): Promise<void>;
    injectServeEvent(event: unknown): Promise<void>;
    applyWebviewSessionState(state: {
      busy: boolean;
      model: string;
      sessionId: string;
    }): Promise<void>;
    sendWebviewDomAction(action: {
      edge?: "bottom" | "top";
      files?: Array<{
        dataBase64: string;
        filename: string;
        mimeType: string;
        sourcePath?: string | null;
      }>;
      index?: number;
      kind:
        | "clickTestId"
        | "dragLeaveTestId"
        | "dragOverTestId"
        | "focusTestId"
        | "pasteClipboardFiles"
        | "pressKeyOnTestId"
        | "probeAttachmentFetch"
        | "probeFullResolutionMemory"
        | "probeImagePipeline"
        | "scrollIntoView"
        | "scrollToEdge"
        | "setInputValue"
        | "setRootWidth";
      scrollBlock?: "center" | "end" | "nearest" | "start";
      testId?: string;
      value?: string;
      widthPx?: number | null;
    }): Promise<void>;
    sendWebviewIntent(intent: {
      data?: Record<string, unknown>;
      messageId: string;
      type: string;
    }): Promise<void>;
    waitForEvent(filter: {
      timeoutMs?: number;
      type?: string;
    }): Promise<unknown>;
    waitForWebviewReady(timeoutMs?: number): Promise<void>;
  };
};

const repoRoot = path.resolve(__dirname, "../../../");
const hostE2e = require(path.resolve(
  repoRoot,
  "out/test/suite/support/hostE2eScenario.js",
)) as {
  getTomcatExtensionApi(): Promise<TomcatExtensionApi>;
};

function requireEnv(name: string): string {
  const value = process.env[name];
  assert.ok(value, `expected ${name} to be defined`);
  return value;
}

async function pause(ms: number): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, ms));
}
/**
 * Runs a narrow-layout check without letting the test-only root width leak into
 * later screenshots when an assertion or wait fails.
 */
async function withRootWidth<T>(
  api: TomcatExtensionApi,
  widthPx: number,
  work: () => Promise<T>,
): Promise<T> {
  await api.__testing.sendWebviewDomAction({ kind: "setRootWidth", widthPx });
  try {
    return await work();
  } finally {
    await api.__testing.sendWebviewDomAction({ kind: "setRootWidth", widthPx: null });
  }
}

async function captureScreenshot(name: string): Promise<string> {
  const targetPath = path.join(requireEnv("TOMCAT_ACCEPT_SCREENSHOTS_DIR"), name);
  await pause(500);
  await (require(path.resolve(repoRoot, "out/test/suite/support/workbenchFindDriver.js")) as {
    captureWorkbenchArtifacts(target: string): Promise<void>;
  }).captureWorkbenchArtifacts(targetPath);
  return targetPath;
}

async function setNextFakeServePromptScript(script: string): Promise<void> {
  const scriptPath = path.join(
    requireEnv("TOMCAT_FAKE_SERVE_STATE_DIR"),
    "manual-acceptance-next-prompt-script.txt",
  );
  await fs.writeFile(scriptPath, `${script}\n`, "utf8");
}

async function waitForDom<T>(
  api: TomcatExtensionApi,
  predicate: (snapshot: DomSnapshot) => T | undefined,
  timeoutMs = 20_000,
): Promise<T> {
  const startedAt = Date.now();
  let lastSnapshot: DomSnapshot | undefined;
  while (Date.now() - startedAt < timeoutMs) {
    lastSnapshot = await api.__testing.captureWebviewDom();
    const result = predicate(lastSnapshot);
    if (result !== undefined) {
      return result;
    }
    await pause(100);
  }
  throw new Error(
    `Timed out waiting for chat DOM: ${JSON.stringify({
      activeSessionId: lastSnapshot?.activeSessionId,
      historyAttachmentThumbCount: lastSnapshot?.historyAttachmentThumbCount,
      historyPdfChipTitles: lastSnapshot?.historyPdfChipTitles,
      messageTexts: lastSnapshot?.messageTexts.slice(-8),
      pendingAttachmentStripClientWidth: lastSnapshot?.pendingAttachmentStripClientWidth,
      pendingAttachmentStripScrollWidth: lastSnapshot?.pendingAttachmentStripScrollWidth,
      pendingAttachmentThumbCount: lastSnapshot?.pendingAttachmentThumbCount,
      timelineKinds: lastSnapshot?.timelineKinds,
    })}`,
  );
}

/**
 * Wait for a condition but never throw — hand back the last snapshot either way.
 *
 * Used where the report itself is the diagnosis. A hard timeout here would abort the run
 * before anything is written to disk, which is exactly when the numbers are most needed:
 * the check still fails at the end, but with evidence attached.
 */
async function pollDom(
  api: TomcatExtensionApi,
  predicate: (snapshot: DomSnapshot) => boolean,
  timeoutMs = 15_000,
): Promise<DomSnapshot> {
  const startedAt = Date.now();
  let snapshot = await api.__testing.captureWebviewDom();
  while (!predicate(snapshot) && Date.now() - startedAt < timeoutMs) {
    await pause(200);
    snapshot = await api.__testing.captureWebviewDom();
  }
  return snapshot;
}

async function waitForPreviewDom<T>(
  api: TomcatExtensionApi,
  predicate: (snapshot: PreviewDomSnapshot) => T | undefined,
  timeoutMs = 20_000,
): Promise<T> {
  const startedAt = Date.now();
  let lastSnapshot: PreviewDomSnapshot | undefined;
  let lastError: unknown;
  while (Date.now() - startedAt < timeoutMs) {
    try {
      lastSnapshot = await api.__testing.captureImagePreviewDom();
      const result = predicate(lastSnapshot);
      if (result !== undefined) {
        return result;
      }
    } catch (error) {
      lastError = error;
    }
    await pause(100);
  }
  throw new Error(
    `Timed out waiting for preview DOM: ${JSON.stringify({
      lastError: lastError instanceof Error ? lastError.message : String(lastError ?? ""),
      lastSnapshot,
    })}`,
  );
}

async function waitForActiveSession(
  api: TomcatExtensionApi,
  timeoutMs = 20_000,
): Promise<string> {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    const state = api.__testing.getWebviewState();
    if (state.activeSessionId && state.sessionViews[state.activeSessionId]) {
      return state.activeSessionId;
    }
    await pause(100);
  }
  throw new Error("Timed out waiting for an active Tomcat session");
}

/**
 * The full-resolution URLs behind the current draft's attachments.
 *
 * Taken from host state rather than the DOM, because the strip deliberately never renders
 * a full-resolution URL any more — which is the whole point, and also why measuring the
 * old behaviour needs the addresses from somewhere else.
 */
function collectPendingFullUris(api: TomcatExtensionApi, sessionId: string): string[] {
  const view = api.__testing.getWebviewState().sessionViews[sessionId] as
    | undefined
    | { pendingAttachments?: Array<{ fullUri?: null | string }> };
  return (view?.pendingAttachments ?? [])
    .map((attachment) => attachment.fullUri ?? null)
    .filter((uri): uri is string => Boolean(uri));
}

/** Intrinsic size of the acceptance fixtures, matching the plan's stated scenario. */
const FIXTURE_WIDTH = 4000;
const FIXTURE_HEIGHT = 3000;

/**
 * One acceptance image, at the size that made this whole redesign necessary.
 *
 * 4000x3000 is deliberate: at four bytes per pixel one of these costs 48MB as a decoded
 * bitmap, so eleven of them is the 528MB the reference-based pipeline exists to avoid.
 * Anything smaller would let a regression through unnoticed.
 */
function createAcceptanceImage(index: number): {
  dataBase64: string;
  filename: string;
  mimeType: "image/svg+xml";
} {
  const hue = (index * 37) % 360;
  const accent = (hue + 155) % 360;
  const number = String(index).padStart(2, "0");
  // Shaped like a Figma or Illustrator export on purpose: a `<style>` block with
  // generated class names, a `style=` attribute, and a gradient reference. Every one of
  // those is what the old attribute blacklist tripped over, so a plain SVG here would
  // pass while the images users actually paste kept being rejected as unsafe.
  const svg = [
    `<svg xmlns="http://www.w3.org/2000/svg" width="${FIXTURE_WIDTH}" height="${FIXTURE_HEIGHT}" viewBox="0 0 480 300">`,
    "<defs>",
    `<linearGradient id="g${number}" x1="0" y1="0" x2="1" y2="1">`,
    `<stop offset="0" stop-color="hsl(${hue} 72% 42%)"/>`,
    `<stop offset="1" stop-color="hsl(${accent} 68% 32%)"/>`,
    "</linearGradient>",
    "</defs>",
    `<style>.cls-1{fill:url(#g${number})}.cls-2{fill:hsl(${accent} 88% 64%);opacity:.9}.cls-3{fill:#fff;font-family:system-ui,sans-serif;font-weight:800}</style>`,
    `<rect class="cls-1" width="480" height="300" rx="28"/>`,
    '<circle class="cls-2" cx="382" cy="82" r="58"/>',
    '<path d="M0 252 L128 126 L230 224 L306 142 L480 300 L0 300 Z" style="fill:rgba(255,255,255,.2);mix-blend-mode:screen"/>',
    '<text class="cls-3" x="32" y="72" style="font-size:28px">TOMCAT IMAGE</text>',
    `<text class="cls-3" x="32" y="210" style="font-size:104px">${number}</text>`,
    "</svg>",
  ].join("");
  return {
    dataBase64: Buffer.from(svg, "utf8").toString("base64"),
    filename: `acceptance-image-${number}.svg`,
    mimeType: "image/svg+xml",
  };
}

/** Bytes a decoded bitmap of this size occupies, at the usual four bytes per pixel. */
function bitmapBytes(width: number, height: number): number {
  return width * height * 4;
}

/**
 * A real, decodable PNG of a solid colour, built here rather than checked in.
 *
 * Needed to tell two failure modes apart. Content-addressed files are named by hash, so
 * they carry no extension, and VS Code derives a webview resource's `Content-Type` from
 * the extension alone. A raster format survives that — browsers sniff the bytes of an
 * `<img>` — while SVG does not, because SVG is only treated as an image when the server
 * says `image/svg+xml`. Testing with SVG fixtures alone cannot distinguish "images are
 * broken" from "SVG is broken".
 */
function createAcceptancePng(index: number): {
  dataBase64: string;
  filename: string;
  mimeType: "image/png";
} {
  const chunk = (type: string, body: Buffer): Buffer => {
    const length = Buffer.alloc(4);
    length.writeUInt32BE(body.length);
    const typed = Buffer.concat([Buffer.from(type, "ascii"), body]);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(typed));
    return Buffer.concat([length, typed, crc]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(FIXTURE_WIDTH, 0);
  ihdr.writeUInt32BE(FIXTURE_HEIGHT, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 2; // truecolour
  // One filter byte plus three bytes per pixel, per row.
  const raw = Buffer.alloc(FIXTURE_HEIGHT * (1 + FIXTURE_WIDTH * 3));
  const red = (index * 37) % 256;
  for (let y = 0; y < FIXTURE_HEIGHT; y += 1) {
    const rowStart = y * (1 + FIXTURE_WIDTH * 3);
    for (let x = 0; x < FIXTURE_WIDTH; x += 1) {
      const at = rowStart + 1 + x * 3;
      raw[at] = red;
      raw[at + 1] = (x * 255) / FIXTURE_WIDTH;
      raw[at + 2] = (y * 255) / FIXTURE_HEIGHT;
    }
  }
  const png = Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", zlib.deflateSync(raw, { level: 6 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
  const number = String(index).padStart(2, "0");
  return {
    dataBase64: png.toString("base64"),
    filename: `acceptance-photo-${number}.png`,
    mimeType: "image/png",
  };
}

function createAcceptancePdf(index: number): {
  dataBase64: string;
  filename: string;
  mimeType: "application/pdf";
} {
  const number = String(index).padStart(2, "0");
  const pdf = Buffer.from(
    `%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n2 0 obj\n<< /Type /Page /Parent 3 0 R >>\nendobj\n3 0 obj\n<< /Type /Pages /Kids [2 0 R] /Count 1 >>\nendobj\ntrailer\n<<>>\n%%EOF\nAcceptance PDF ${number}\n`,
    "utf8",
  );
  return {
    dataBase64: pdf.toString("base64"),
    filename: `acceptance-brief-${number}.pdf`,
    mimeType: "application/pdf",
  };
}

const CRC_TABLE = (() => {
  const table = new Int32Array(256);
  for (let n = 0; n < 256; n += 1) {
    let c = n;
    for (let k = 0; k < 8; k += 1) {
      c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    }
    table[n] = c;
  }
  return table;
})();

function crc32(buffer: Buffer): number {
  let crc = -1;
  for (const byte of buffer) {
    crc = CRC_TABLE[(crc ^ byte) & 0xff]! ^ (crc >>> 8);
  }
  return (crc ^ -1) >>> 0;
}

async function setColorTheme(
  themeName: string,
  expectedKind: vscode.ColorThemeKind,
): Promise<boolean> {
  await vscode.workspace
    .getConfiguration("workbench")
    .update("colorTheme", themeName, vscode.ConfigurationTarget.Global);
  const startedAt = Date.now();
  while (Date.now() - startedAt < 10_000) {
    if (vscode.window.activeColorTheme.kind === expectedKind) {
      await pause(500);
      return true;
    }
    await pause(100);
  }
  return false;
}

async function sendDomAction(
  api: TomcatExtensionApi,
  action: Parameters<TomcatExtensionApi["__testing"]["sendWebviewDomAction"]>[0],
): Promise<void> {
  await api.__testing.sendWebviewDomAction(action);
  await pause(200);
}

suite("Tomcat image attachment visual acceptance", () => {
  test("captures real-host image attachment artifacts", async () => {
    const reportPath = requireEnv("TOMCAT_ACCEPT_REPORT_PATH");
    const screenshots: string[] = [];
    const originalColorTheme = vscode.workspace
      .getConfiguration("workbench")
      .get<string>("colorTheme");
    const api = await hostE2e.getTomcatExtensionApi();
    let blockedInlineDir: string | null = null;

    try {
      await api.__testing.focusWebview();
      await api.__testing.waitForWebviewReady(20_000);
      const sessionId = await waitForActiveSession(api);

      // Runs first: everything below assumes Chromium can actually downsample and
      // rasterise in this environment, and this is the only place that proves it.
      await sendDomAction(api, { kind: "probeImagePipeline" });
      const pipelineProbe = await waitForDom(
        api,
        (snapshot) => {
          if (!snapshot.imagePipelineProbe) return undefined;
          const parsed = JSON.parse(snapshot.imagePipelineProbe) as PipelineProbeResult[];
          return parsed.length === 3 ? parsed : undefined;
        },
        30_000,
      );

      // Ten photos and one SVG. The mix is deliberate: rasters and SVG take different
      // paths through the resource protocol's MIME handling, and only running both tells
      // us which one is failing when an image comes up blank.
      const images = [
        createAcceptancePng(1),
        ...Array.from({ length: 9 }, (_, index) => createAcceptancePng(index + 2)),
        createAcceptanceImage(11),
      ];

      await api.__testing.sendWebviewIntent({
        data: {
          files: [images[0]],
          sessionId,
        },
        messageId: "image-acceptance-attach-single",
        type: "attachFiles",
      });
      const singleDraft = await waitForDom(
        api,
        (snapshot) =>
          snapshot.pendingAttachmentThumbCount === 1 ? snapshot : undefined,
        30_000,
      );
      screenshots.push(await captureScreenshot("01-composer-single-image.png"));

      await api.__testing.sendWebviewIntent({
        data: {
          files: images.slice(1),
          sessionId,
        },
        messageId: "image-acceptance-attach-rest",
        type: "attachFiles",
      });
      const { narrowDraft, skeletonDraft } = await withRootWidth(api, 180, async () => {
        // Thumbnails are generated one at a time, so for a moment the strip is a mix of
        // finished images and placeholders. Worth a screenshot: this is the state a user
        // sees right after dropping a folder of photos, and the placeholders are exactly
        // the size of the thumbnails that replace them, so nothing shifts.
        // Caught as early as placeholders exist at all. Thumbnails are generated left to
        // right, so the leftmost squares — the only ones a narrow sidebar shows — are the
        // first to be replaced; waiting for all eleven attachments to land first would leave
        // nothing of this state visible in the screenshot.
        const skeletonDraft = await pollDom(
          api,
          (snapshot) => snapshot.attachmentSkeletonCount >= 3,
          8_000,
        );
        if (skeletonDraft.attachmentSkeletonCount > 0) {
          // Scroll to the *last* placeholder. The leftmost squares are replaced within a
          // second of arriving, so by the time a screenshot is taken they are finished
          // images; the far end of the strip is still waiting and is what this shot is for.
          await sendDomAction(api, {
            index: -1,
            kind: "scrollIntoView",
            scrollBlock: "nearest",
            testId: "attachment-skeleton",
          });
          screenshots.push(await captureScreenshot("19-composer-thumbnail-skeleton.png"));
        }

        const narrowDraft = await waitForDom(
          api,
          (snapshot) =>
            snapshot.pendingAttachmentThumbCount === 11 &&
            snapshot.pendingAttachmentStripOverflowing
              ? snapshot
              : undefined,
          60_000,
        );
        screenshots.push(await captureScreenshot("02-composer-11-images-narrow.png"));
        return { narrowDraft, skeletonDraft };
      });
      // What the resource protocol actually served, so a broken image cannot pass as a
      // rendered one: this is the single check that keeps the whole asWebviewUri path
      // honest.
      await sendDomAction(api, { kind: "probeAttachmentFetch" });
      const fetchProbe = await waitForDom(api, (snapshot) =>
        snapshot.attachmentFetchProbe ? snapshot : undefined,
      );
      const attachmentDiagnostics = api.__testing.getAttachmentDiagnostics();
      const attachmentFetch = JSON.parse(fetchProbe.attachmentFetchProbe!) as {
        bytes?: number;
        contentType?: string | null;
        error?: string;
        naturalWidth: number | null;
        ok?: boolean;
        status?: number;
      };
      const lazyStrip = await pollDom(
        api,
        (snapshot) =>
          snapshot.attachmentBitmaps.length === 11 &&
          snapshot.attachmentBitmaps.some((bitmap) => bitmap.width > 0),
      );
      for (let index = 0; index < 11; index += 1) {
        await sendDomAction(api, {
          index,
          kind: "scrollIntoView",
          scrollBlock: "nearest",
          testId: "attachment-thumb",
        });
      }
      const loadedStrip = await pollDom(
        api,
        (snapshot) =>
          snapshot.attachmentBitmaps.length === 11 &&
          snapshot.attachmentBitmaps.every((bitmap) => bitmap.width > 0),
      );
      screenshots.push(await captureScreenshot("22-composer-thumb-sharpness.png"));

      // ── The same measurement, taken the old way ───────────────────────────────────
      // The strip used to point at the full-resolution image. Rather than multiplying
      // 4000x3000 on paper and calling it the "before" number, render the same eleven
      // images the old way in the same webview and read back what Chromium decoded, so
      // both halves of the comparison are measurements. The host's own memory is sampled
      // around it too, to show that the bytes are no longer passing through it.
      const attachmentUris = collectPendingFullUris(api, sessionId);
      const hostRssBefore = process.memoryUsage().rss;
      await sendDomAction(api, {
        kind: "probeFullResolutionMemory",
        value: JSON.stringify(attachmentUris),
      });
      const fullResolutionSample = await pollDom(
        api,
        (snapshot) => Boolean(snapshot.fullResolutionProbe),
        60_000,
      );
      const hostRssAfter = process.memoryUsage().rss;
      const fullResolutionMeasured = JSON.parse(
        fullResolutionSample.fullResolutionProbe ?? "null",
      ) as null | {
        bitmaps: Array<{ height: number; width: number }>;
        bytes: number;
        failures: string[];
        measured: number;
        requested: number;
      };

      // ── Keyboard reach ────────────────────────────────────────────────────────────
      await sendDomAction(api, {
        index: 0,
        kind: "focusTestId",
        testId: "attachment-thumb",
      });
      const focusedStrip = await pollDom(
        api,
        (snapshot) => snapshot.focusedTestId === "attachment-thumb",
      );
      screenshots.push(await captureScreenshot("16-composer-keyboard-focus.png"));

      // ── The draft outlives the backend ────────────────────────────────────────────
      // The point of moving draft ownership out of Rust: killing serve must not cost the
      // user their unsent text or their eleven attachments.
      const draftText = "Draft that must survive a serve restart";
      await sendDomAction(api, {
        kind: "setInputValue",
        testId: "composer-input",
        value: draftText,
      });
      await waitForDom(api, (snapshot) =>
        snapshot.composerText?.includes(draftText) ? snapshot : undefined,
      );
      await api.__testing.restartServe();
      await api.__testing.waitForWebviewReady(20_000);
      await api.__testing.focusWebview();
      const draftAfterRestart = await pollDom(
        api,
        (snapshot) =>
          snapshot.pendingAttachmentThumbCount === 11 &&
          snapshot.composerText?.includes(draftText) === true,
        25_000,
      );
      screenshots.push(await captureScreenshot("17-draft-survives-serve-restart.png"));

      // Surviving is half the requirement: the recovered draft has to still be a live
      // draft. Typing more into it and reloading proves the editing path came back too,
      // rather than the composer being left showing a read-only echo of the old text.
      // Appended, because the composer is a rich-text editor and this action types into
      // it rather than replacing its contents.
      await sendDomAction(api, {
        kind: "setInputValue",
        testId: "composer-input",
        value: " and still editable",
      });
      await pollDom(api, (snapshot) =>
        Boolean(snapshot.composerText?.includes("still editable")),
      );
      // Draft writes are debounced, so reloading the instant the DOM updates would read
      // back the previous version and make a working draft look broken.
      await pause(1_500);
      await api.__testing.reloadWebview();
      await api.__testing.waitForWebviewReady(20_000);
      await api.__testing.focusWebview();
      const draftAfterEditing = await pollDom(
        api,
        (snapshot) =>
          snapshot.composerText?.includes("still editable") === true &&
          snapshot.pendingAttachmentThumbCount === 11,
        25_000,
      );

      await sendDomAction(api, {
        index: 1,
        kind: "clickTestId",
        testId: "attachment-thumb",
      });
      const pendingPreview = await waitForPreviewDom(
        api,
        (snapshot) =>
          snapshot.position === 2 && snapshot.total === 11
            ? snapshot
            : undefined,
      );
      screenshots.push(await captureScreenshot("03-preview-pending-02-of-11.png"));
      screenshots.push(await captureScreenshot("09-preview-toolbar-dark.png"));

      const previewLightThemePassed = await setColorTheme(
        "Default Light Modern",
        vscode.ColorThemeKind.Light,
      );
      await pause(300);
      screenshots.push(await captureScreenshot("09-preview-toolbar-light.png"));
      await setColorTheme("Default Dark Modern", vscode.ColorThemeKind.Dark);
      await pause(300);

      await api.__testing.dispatchImagePreviewDomAction({ kind: "copy" });
      const copiedPreview = await waitForPreviewDom(
        api,
        (snapshot) =>
          snapshot.copyButtonCopied === true &&
          /codicon-check/u.test(snapshot.copyIconClass ?? "")
            ? snapshot
            : undefined,
      );
      screenshots.push(await captureScreenshot("10-preview-copy-copied.png"));

      await api.__testing.dispatchImagePreviewDomAction({ kind: "zoomIn" });
      const zoomedPreview = await waitForPreviewDom(
        api,
        (snapshot) =>
          typeof snapshot.zoom === "number" && snapshot.zoom > 1
            ? snapshot
            : undefined,
      );
      screenshots.push(await captureScreenshot("04-preview-zoomed.png"));

      await api.__testing.executeCommand("workbench.action.closeActiveEditor");
      await api.__testing.focusWebview();
      await sendDomAction(api, { kind: "setRootWidth", widthPx: null });
      api.__testing.clearObservedEvents();
      await api.__testing.sendWebviewIntent({
        data: {
          sessionId,
          text: "Image acceptance history message",
        },
        messageId: "image-acceptance-prompt",
        type: "prompt",
      });
      await api.__testing.waitForEvent({ timeoutMs: 20_000, type: "agent_end" });
      const sentHistory = await waitForDom(
        api,
        (snapshot) =>
          snapshot.historyAttachmentThumbCount >= 11 ? snapshot : undefined,
      );
      screenshots.push(await captureScreenshot("05-history-images-sent.png"));

      await api.__testing.restartServe();
      await api.__testing.waitForWebviewReady(20_000);
      await api.__testing.focusWebview();
      const restartedHistory = await waitForDom(
        api,
        (snapshot) =>
          snapshot.historyAttachmentThumbCount >= 11 ? snapshot : undefined,
        25_000,
      );
      screenshots.push(await captureScreenshot("06-history-images-after-restart.png"));

      // Restarting serve leaves the host's rendered history in memory. Reloading the
      // webview throws it away and rebuilds it from the transcript, which is what happens
      // on a session switch or a reopened window — and a transcript stores hashes with no
      // URLs, so this is the path where history images can come back address-less and sit
      // as placeholders forever.
      await api.__testing.reloadWebview();
      await api.__testing.waitForWebviewReady(20_000);
      await api.__testing.focusWebview();
      const rebuiltHistory = await pollDom(
        api,
        (snapshot) => snapshot.historyAttachmentThumbCount >= 11,
        30_000,
      );

      await sendDomAction(api, {
        index: 1,
        kind: "clickTestId",
        testId: "history-attachment-thumb",
      });
      const historyPreview = await waitForPreviewDom(
        api,
        (snapshot) =>
          snapshot.position === 2 && snapshot.total === 11
            ? snapshot
            : undefined,
      );
      screenshots.push(await captureScreenshot("07-preview-history-02-of-11.png"));

      // The last fixture is the design-tool SVG. It reaches the stage through its own
      // route — refetched into a typed blob, because a hash-named resource carries no
      // extension for VS Code to derive `image/svg+xml` from — so it is the one picture
      // that can fail while the ten rasters around it look perfect.
      for (let step = 0; step < 12; step += 1) {
        const seen = await waitForPreviewDom(api, (snapshot) => snapshot);
        if (seen.position === 11) break;
        await api.__testing.dispatchImagePreviewDomAction({ kind: "next" });
        await pause(120);
      }
      const svgPreview = await waitForPreviewDom(
        api,
        (snapshot) =>
          snapshot.position === 11 && snapshot.stageNaturalWidth > 0
            ? snapshot
            : undefined,
      ).catch(() => api.__testing.captureImagePreviewDom());
      screenshots.push(await captureScreenshot("20-preview-svg-attachment.png"));

      await api.__testing.executeCommand("workbench.action.closeActiveEditor");
      await api.__testing.focusWebview();

      const lightThemePassed = await setColorTheme(
        "Default Light Modern",
        vscode.ColorThemeKind.Light,
      );
      await api.__testing.focusWebview();
      screenshots.push(await captureScreenshot("08-history-light-theme.png"));

      const highContrastThemePassed = await setColorTheme(
        "Default High Contrast",
        vscode.ColorThemeKind.HighContrast,
      );
      await api.__testing.focusWebview();
      screenshots.push(await captureScreenshot("21-history-high-contrast.png"));

      await setColorTheme("Default Dark Modern", vscode.ColorThemeKind.Dark);
      await api.__testing.focusWebview();

      // ── Bytes gone from under a draft ─────────────────────────────────────────────
      // The extension holds hashes, the backend holds bytes, and the two can drift: a
      // deleted tomcat home, a draft file synced to another machine. That has to read as
      // a removable attachment, never as a broken image.
      const strandedImage = createAcceptanceImage(12);
      await api.__testing.sendWebviewIntent({
        data: {
          files: [strandedImage],
          sessionId,
        },
        messageId: "image-acceptance-attach-stranded",
        type: "attachFiles",
      });
      await pollDom(api, (snapshot) => snapshot.pendingAttachmentItemCount === 1, 30_000);
      const blobsDir = path.join(
        requireEnv("TOMCAT_FAKE_SERVE_STATE_DIR"),
        "attachments",
        "blobs",
      );
      for (const entry of await fs.readdir(blobsDir)) {
        await fs.rm(path.join(blobsDir, entry));
      }
      // Reloading is what re-reads the draft and re-checks the blobs behind it, which is
      // also how a user would meet this state: a draft written on one machine, opened on
      // another where the bytes were never synced.
      await api.__testing.restartServe();
      await api.__testing.reloadWebview();
      await api.__testing.waitForWebviewReady(20_000);
      await api.__testing.focusWebview();
      const strandedDraft = await pollDom(
        api,
        (snapshot) => snapshot.attachmentUnavailableCount >= 1,
        25_000,
      );
      screenshots.push(await captureScreenshot("18-attachment-unavailable.png"));

      // One click clears the dead attachment, and the rest of the draft is untouched.
      await sendDomAction(api, {
        index: 0,
        kind: "clickTestId",
        testId: "attachment-remove",
      });
      const clearedStranded = await pollDom(
        api,
        (snapshot) => snapshot.attachmentUnavailableCount === 0,
      );

      // ── Removal without a mouse ───────────────────────────────────────────────────
      // Clicking a thumbnail opens the preview, so Delete on a focused thumbnail is the
      // only keyboard path to removing one.
      await api.__testing.sendWebviewIntent({
        data: {
          files: [createAcceptanceImage(13)],
          sessionId,
        },
        messageId: "image-acceptance-attach-remove-keyboard",
        type: "attachFiles",
      });
      // Waits for the thumbnail, not just the attachment: until one exists the strip shows
      // a placeholder, and a placeholder is not a focusable target.
      await pollDom(api, (snapshot) => snapshot.pendingAttachmentThumbCount === 1, 30_000);
      await sendDomAction(api, {
        index: 0,
        kind: "pressKeyOnTestId",
        testId: "attachment-thumb",
        value: "Delete",
      });
      const clearedByKeyboard = await pollDom(
        api,
        (snapshot) => snapshot.pendingAttachmentItemCount === 0,
      );

      // ── PDF paste path and transcript inline images ──────────────────────────────
      const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
      assert.ok(workspaceRoot, "expected a workspace root for PDF and inline-image acceptance");
      const inlineRelativePath = "docs/accept-inline-image.png";
      const inlineWorkspacePath = path.join(workspaceRoot, inlineRelativePath);
      await fs.mkdir(path.dirname(inlineWorkspacePath), { recursive: true });
      await fs.writeFile(
        inlineWorkspacePath,
        Buffer.from(createAcceptancePng(21).dataBase64, "base64"),
      );

      blockedInlineDir = await fs.mkdtemp(
        path.join(os.homedir(), ".tomcat-inline-image-blocked-"),
      );
      const blockedInlinePath = path.join(blockedInlineDir, "blocked-inline-image.png");
      await fs.writeFile(
        blockedInlinePath,
        Buffer.from(createAcceptancePng(22).dataBase64, "base64"),
      );

      const pdfRelativePath = "docs/acceptance-brief-01.pdf";
      const pdfWorkspacePath = path.join(workspaceRoot, pdfRelativePath);
      const pdfAttachment = {
        ...createAcceptancePdf(1),
        sourcePath: pdfWorkspacePath,
      };
      await fs.writeFile(pdfWorkspacePath, Buffer.from(pdfAttachment.dataBase64, "base64"));

      await sendDomAction(api, {
        files: [pdfAttachment],
        kind: "pasteClipboardFiles",
        testId: "composer-input",
      });
      const pendingPdfChip = await waitForDom(
        api,
        (snapshot) =>
          snapshot.pendingPdfChipTitles.includes(pdfWorkspacePath)
            ? snapshot
            : undefined,
        20_000,
      );
      screenshots.push(await captureScreenshot("11-composer-pdf-chip.png"));

      api.__testing.clearObservedEvents();
      await api.__testing.sendWebviewIntent({
        data: {
          sessionId,
          text: "PDF acceptance history message",
        },
        messageId: "image-acceptance-pdf-prompt",
        type: "prompt",
      });
      await api.__testing.waitForEvent({ timeoutMs: 20_000, type: "agent_end" });
      const historyPdfChip = await waitForDom(
        api,
        (snapshot) =>
          // The draft owns the local source path, but the serve protocol only
          // persists a filename/hash in transcript history. Requiring an
          // absolute workstation path here would both contradict that boundary
          // and falsely fail a correct reload of the same file attachment.
          snapshot.historyPdfChipTitles.some((title) =>
            title.startsWith(`${pdfAttachment.filename} ·`),
          )
            ? snapshot
            : undefined,
        20_000,
      );
      screenshots.push(await captureScreenshot("12-history-pdf-chip.png"));

      await api.__testing.applyWebviewSessionState({
        busy: true,
        model: "gpt-5.4",
        sessionId,
      });
      await api.__testing.injectServeEvent({
        sessionId,
        type: "agent_start",
      });
      await api.__testing.injectServeEvent({
        assistantMessageEvent: {
          delta: [
            "Workspace image:\n\n",
            `![workspace image](${inlineRelativePath})`,
            "\n\nBlocked image:\n\n",
            `![blocked image](${blockedInlinePath})`,
          ].join(""),
          kind: "content_delta",
        },
        assistantMessageId: "assistant-inline-image-acceptance",
        message: {},
        sessionId,
        type: "message_update",
      });
      const inlineImageMessage = await waitForDom(
        api,
        (snapshot) =>
          snapshot.inlineImages.some((image) => image.naturalWidth > 0) &&
          snapshot.blockedInlineImageTexts.some((text) => text.includes(blockedInlinePath))
            ? snapshot
            : undefined,
        20_000,
      );
      await api.__testing.injectServeEvent({
        assistantMessageId: "assistant-inline-image-acceptance",
        message: {},
        sessionId,
        type: "message_end",
      });
      await api.__testing.injectServeEvent({
        messages: [],
        sessionId,
        type: "agent_end",
      });
      await api.__testing.injectServeEvent({
        sessionId,
        type: "agent_idle",
      });
      await api.__testing.applyWebviewSessionState({
        busy: false,
        model: "gpt-5.4",
        sessionId,
      });
      screenshots.push(await captureScreenshot("13-transcript-inline-image.png"));

      await sendDomAction(api, {
        index: 0,
        kind: "clickTestId",
        testId: "inline-image",
      });
      const inlineImageLightbox = await waitForDom(
        api,
        (snapshot) =>
          snapshot.lightboxVisible && snapshot.lightboxImageNaturalWidth > 0
            ? snapshot
            : undefined,
        20_000,
      );
      screenshots.push(await captureScreenshot("14-inline-image-lightbox.png"));
      await sendDomAction(api, {
        kind: "clickTestId",
        testId: "image-lightbox-overlay",
      });
      const inlineImageLightboxClosed = await waitForDom(
        api,
        (snapshot) => (!snapshot.lightboxVisible ? snapshot : undefined),
        10_000,
      );

      await setNextFakeServePromptScript("terminal_llm_error");
      api.__testing.clearObservedEvents();
      await api.__testing.sendWebviewIntent({
        data: {
          sessionId,
          text: "terminal llm error acceptance",
        },
        messageId: "image-acceptance-terminal-llm-error",
        type: "prompt",
      });
      await api.__testing.waitForEvent({ timeoutMs: 20_000, type: "agent_end" });
      const terminalLlmErrorSnapshot = await waitForDom(
        api,
        (snapshot) =>
          snapshot.messageTexts.filter(
            (text) => text === "Gateway rejected the attachment payload",
          ).length === 1
            ? snapshot
            : undefined,
        20_000,
      );

      await api.__testing.sendWebviewIntent({
        messageId: "image-acceptance-retry-notice-session",
        type: "newSession",
      });
      const retryNoticeSessionId = await waitForDom(
        api,
        (snapshot) =>
          snapshot.activeSessionId && snapshot.activeSessionId !== sessionId
            ? snapshot.activeSessionId
            : undefined,
        20_000,
      );
      await setNextFakeServePromptScript("llm_error_degrade_then_success");
      api.__testing.clearObservedEvents();
      await api.__testing.sendWebviewIntent({
        data: {
          sessionId: retryNoticeSessionId,
          text: "retry notice acceptance",
        },
        messageId: "image-acceptance-retry-degrade-success",
        type: "prompt",
      });
      const retryNoticeSnapshot = await waitForDom(
        api,
        (snapshot) =>
          snapshot.activeSessionId === retryNoticeSessionId
            && snapshot.messageTexts.some((text) => text.includes("Retrying after error:"))
            && snapshot.messageTexts.some((text) =>
              text.includes("本轮附件未被当前端点接受，已按纯文本发送"),
            )
            ? snapshot
            : undefined,
        20_000,
      );
      screenshots.push(await captureScreenshot("15-retry-and-degrade-notices.png"));
      await api.__testing.waitForEvent({ timeoutMs: 20_000, type: "agent_end" });
      const retrySuccessSnapshot = await waitForDom(
        api,
        (snapshot) =>
          snapshot.activeSessionId === retryNoticeSessionId
            && snapshot.messageTexts.some((text) =>
              text.includes("Scripted retry success for prompt: retry notice acceptance."),
            )
            && !snapshot.messageTexts.some((text) =>
              text.includes("Gateway rejected the attachment payload"),
            )
            ? snapshot
            : undefined,
        20_000,
      );

      const probeFor = (label: string): PipelineProbeResult =>
        pipelineProbe.find((entry) => entry.label === label) ?? { label };

      const fullResolutionBytes =
        loadedStrip.attachmentBitmaps.length *
        bitmapBytes(FIXTURE_WIDTH, FIXTURE_HEIGHT);
      const decodedOnArrival = lazyStrip.attachmentBitmaps.filter(
        (bitmap) => bitmap.width > 0,
      ).length;
      const eagerDecodeStillBounded =
        lazyStrip.attachmentBitmapBytes > 0 &&
        lazyStrip.attachmentBitmapBytes < fullResolutionBytes / 100;

      const report = {
        artifactsRoot: path.dirname(reportPath),
        observations: {
          // Chromium decides exactly when offscreen horizontal images decode. Some
          // builds defer them, others eagerly decode them, so treating that timing as
          // pass/fail would turn this acceptance run into a browser-version detector.
          attachmentLazyDecode: {
            decodedOnArrival,
            measuredBytes: lazyStrip.attachmentBitmapBytes,
            offscreenStillDeferred:
              decodedOnArrival < lazyStrip.attachmentBitmaps.length,
            total: lazyStrip.attachmentBitmaps.length,
          },
        },
        checks: {
          composerSingle: {
            passed: singleDraft.pendingAttachmentThumbCount === 1,
          },
          // VS Code derives a webview resource's Content-Type from the file extension
          // alone, and content-addressed blobs are named by hash. If that leaves the
          // browser unable to render the bytes, images silently fall back to alt text.
          attachmentResourceFetch: {
            details: attachmentFetch,
            host: attachmentDiagnostics,
            passed: attachmentFetch.ok === true && (attachmentFetch.naturalWidth ?? 0) > 0,
          },
          // Hard requirement: whether Chromium decodes eagerly or lazily, it must only
          // decode bounded 192px thumbnails here, never the original 4000x3000 sources.
          attachmentBitmapBoundedOnArrival: {
            decodedOnArrival,
            measuredBytes: lazyStrip.attachmentBitmapBytes,
            passed: eagerDecodeStillBounded,
            total: lazyStrip.attachmentBitmaps.length,
          },
          // The headline number: what eleven 4000x3000 images cost in the strip, measured
          // from the sizes Chromium decoded rather than asserted from the source files.
          attachmentBitmapMemory: {
            bitmaps: loadedStrip.attachmentBitmaps,
            fullResolutionBytes,
            measuredBytes: loadedStrip.attachmentBitmapBytes,
            passed:
              loadedStrip.attachmentBitmapBytes > 0 &&
              loadedStrip.attachmentBitmapBytes < fullResolutionBytes / 100 &&
              loadedStrip.attachmentBitmaps.every(
                (bitmap) =>
                  bitmap.resolution === "thumb" &&
                  Math.max(bitmap.width, bitmap.height) <= 192,
              ),
            reductionFactor:
              loadedStrip.attachmentBitmapBytes > 0
                ? Math.round(fullResolutionBytes / loadedStrip.attachmentBitmapBytes)
                : null,
            sourcePixels: `${FIXTURE_WIDTH}x${FIXTURE_HEIGHT}`,
          },
          // Both sides measured, on the same machine, on the same eleven images: what the
          // old strip cost when it pointed at the originals, against what it costs now.
          memoryBaselineComparison: {
            afterMeasuredBytes: loadedStrip.attachmentBitmapBytes,
            beforeBitmaps: fullResolutionMeasured?.bitmaps ?? [],
            beforeMeasuredBytes: fullResolutionMeasured?.bytes ?? null,
            // The host maps paths and never reads the bytes, so its own footprint should
            // barely move while the renderer decodes half a gigabyte.
            hostRssDeltaBytes: hostRssAfter - hostRssBefore,
            imagesMeasured: fullResolutionMeasured?.measured ?? 0,
            imagesRequested: fullResolutionMeasured?.requested ?? 0,
            // Ten of the eleven fixtures are PNGs. The SVG cannot be decoded straight
            // from a hash-named URL — that is the documented reason it goes through a
            // typed blob URL instead — so ten measured images is the full result here.
            measurementFailures: fullResolutionMeasured?.failures ?? [],
            passed:
              (fullResolutionMeasured?.measured ?? 0) >= 10 &&
              (fullResolutionMeasured?.bytes ?? 0) >
                loadedStrip.attachmentBitmapBytes * 100,
            reductionFactor:
              fullResolutionMeasured && loadedStrip.attachmentBitmapBytes > 0
                ? Math.round(
                    fullResolutionMeasured.bytes / loadedStrip.attachmentBitmapBytes,
                  )
                : null,
          },
          // A 48px box drawn from a 192px source stays sharp on a 2x display.
          thumbnailSharpness: {
            cssBoxPx: 48,
            passed: loadedStrip.attachmentBitmaps.every(
              (bitmap) => Math.max(bitmap.width, bitmap.height) >= 144,
            ),
            sourceEdges: loadedStrip.attachmentBitmaps.map((bitmap) =>
              Math.max(bitmap.width, bitmap.height),
            ),
          },
          keyboardFocus: {
            focusedTestId: focusedStrip.focusedTestId,
            passed: focusedStrip.focusedTestId === "attachment-thumb",
          },
          keyboardRemoval: {
            passed: clearedByKeyboard.pendingAttachmentItemCount === 0,
          },
          previewCodiconFont: {
            fontFamily: copiedPreview.downloadIconFontFamily ?? null,
            passed: /codicon/i.test(copiedPreview.downloadIconFontFamily ?? ""),
          },
          previewCopyFeedback: {
            copied: copiedPreview.copyButtonCopied === true,
            iconClass: copiedPreview.copyIconClass ?? null,
            passed:
              copiedPreview.copyButtonCopied === true &&
              /codicon-check/u.test(copiedPreview.copyIconClass ?? ""),
          },
          pdfPastePath: {
            passed: pendingPdfChip.pendingPdfChipTitles.includes(pdfWorkspacePath),
            titles: pendingPdfChip.pendingPdfChipTitles,
          },
          pdfHistoryRendering: {
            passed: historyPdfChip.historyPdfChipTitles.some((title) =>
              title.startsWith(`${pdfAttachment.filename} ·`),
            ),
            titles: historyPdfChip.historyPdfChipTitles,
          },
          // Placeholders while thumbnails are generated, rather than originals: the
          // in-between state has to be bounded too.
          thumbnailSkeleton: {
            observed: skeletonDraft.attachmentSkeletonCount,
            // Placeholders while thumbnails are generated, each exactly the size of the
            // thumbnail that replaces it, and eleven finished images by the end.
            passed:
              skeletonDraft.attachmentSkeletonCount >= 3 &&
              narrowDraft.pendingAttachmentItemCount === 11,
          },
          // Killing the backend must not cost the user unsent work, and the recovered
          // draft has to still accept edits that themselves survive a reload.
          draftSurvivesServeRestart: {
            editableAfterRestart:
              draftAfterEditing.composerText?.includes("still editable") === true,
            passed:
              draftAfterRestart.pendingAttachmentThumbCount === 11 &&
              draftAfterRestart.composerText?.includes(draftText) === true &&
              draftAfterEditing.composerText?.includes("still editable") === true &&
              draftAfterEditing.pendingAttachmentThumbCount === 11,
            text: draftAfterRestart.composerText,
            textAfterEditing: draftAfterEditing.composerText,
            thumbCount: draftAfterRestart.pendingAttachmentThumbCount,
          },
          // Missing bytes degrade to a removable chip, not a broken image.
          attachmentUnavailableState: {
            clearedAfterRemove: clearedStranded.attachmentUnavailableCount === 0,
            // Deleting the blob store strands the sent images too. Those are a record of
            // something that happened, so they stay visible as unavailable rather than
            // disappearing or hanging on a placeholder that will never load.
            historyUnavailableCount: strandedDraft.historyAttachmentUnavailableCount,
            passed:
              strandedDraft.attachmentUnavailableCount >= 1 &&
              strandedDraft.historyAttachmentUnavailableCount >= 11 &&
              clearedStranded.attachmentUnavailableCount === 0,
            unavailableCount: strandedDraft.attachmentUnavailableCount,
          },
          // The canvas route: does Chromium in a real webview rasterise SVG to PNG,
          // including an SVG that declares no size, without tainting the canvas?
          imagePipelineCanvas: {
            details: pipelineProbe,
            passed: ["svg-with-size", "svg-without-size", "svg-design-tool"].every(
              (label) => {
                const probe = probeFor(label);
                return (
                  probe.error === undefined &&
                  probe.rasterised === true &&
                  probe.providerIsPng === true &&
                  probe.usedSourceFallback !== true &&
                  (probe.providerSize?.width ?? 0) > 0
                );
              },
            ),
          },
          // Decode-time downsampling: every thumbnail's longest edge within 192px.
          imagePipelineThumbnails: {
            passed: pipelineProbe.every((probe) => probe.thumbWithinBudget === true),
            sizes: pipelineProbe.map((probe) => ({
              label: probe.label,
              thumbSize: probe.thumbSize,
            })),
          },
          composerNarrowOverflow: {
            clientWidth: narrowDraft.pendingAttachmentStripClientWidth,
            passed:
              narrowDraft.pendingAttachmentThumbCount === 11 &&
              narrowDraft.pendingAttachmentStripOverflowing &&
              narrowDraft.pendingAttachmentStripScrollWidth >
                narrowDraft.pendingAttachmentStripClientWidth,
            scrollWidth: narrowDraft.pendingAttachmentStripScrollWidth,
          },
          historyPreview: {
            activeThumbIndex: historyPreview.activeThumbIndex,
            passed:
              historyPreview.position === 2 &&
              historyPreview.total === 11 &&
              historyPreview.activeThumbIndex === 1,
          },
          historyRendering: {
            passed: sentHistory.historyAttachmentThumbCount >= 11,
            thumbCount: sentHistory.historyAttachmentThumbCount,
          },
          // Rebuilt from the transcript rather than from the host's in-memory copy, which
          // is the only path where hashes have to be turned back into URLs.
          historyRebuiltFromTranscript: {
            passed: rebuiltHistory.historyAttachmentThumbCount >= 11,
            thumbCount: rebuiltHistory.historyAttachmentThumbCount,
          },
          inlineWorkspaceImage: {
            cursor: inlineImageMessage.inlineImages[0]?.cursor ?? null,
            naturalWidths: inlineImageMessage.inlineImages.map((image) => image.naturalWidth),
            passed:
              inlineImageMessage.inlineImages.some((image) => image.naturalWidth > 0) &&
              inlineImageMessage.inlineImages.some((image) => image.cursor === "zoom-in"),
          },
          inlineOutsideImageBlocked: {
            blockedTexts: inlineImageMessage.blockedInlineImageTexts,
            passed: inlineImageMessage.blockedInlineImageTexts.some((text) =>
              text.includes(blockedInlinePath),
            ),
          },
          inlineImageLightbox: {
            lightboxImageNaturalWidth: inlineImageLightbox.lightboxImageNaturalWidth,
            lightboxImageSrc: inlineImageLightbox.lightboxImageSrc,
            passed:
              inlineImageLightbox.lightboxVisible &&
              inlineImageLightbox.lightboxImageNaturalWidth > 0 &&
              !inlineImageLightboxClosed.lightboxVisible,
          },
          terminalLlmErrorDedupe: {
            matches: terminalLlmErrorSnapshot.messageTexts.filter(
              (text) => text === "Gateway rejected the attachment payload",
            ).length,
            passed:
              terminalLlmErrorSnapshot.messageTexts.filter(
                (text) => text === "Gateway rejected the attachment payload",
              ).length === 1,
          },
          retryNoticeAndDegradeState: {
            passed:
              retryNoticeSnapshot.messageTexts.some((text) =>
                text.includes("Retrying after error:"),
              ) &&
              retryNoticeSnapshot.messageTexts.some((text) =>
                text.includes("本轮附件未被当前端点接受，已按纯文本发送"),
              ),
            timelineKinds: retryNoticeSnapshot.timelineKinds,
          },
          retrySuccessNoResidualError: {
            passed:
              retrySuccessSnapshot.messageTexts.some((text) =>
                text.includes("Scripted retry success for prompt: retry notice acceptance."),
              ) &&
              !retrySuccessSnapshot.messageTexts.some((text) =>
                text.includes("Gateway rejected the attachment payload"),
              ),
            messageTexts: retrySuccessSnapshot.messageTexts,
          },
          // A design-tool SVG — `<style>` block, generated class names, `style=`
          // attributes — displayed as a vector, not as the PNG that exists for the model.
          svgAttachmentRenders: {
            passed: svgPreview.position === 11 && svgPreview.stageNaturalWidth > 0,
            position: svgPreview.position,
            stageNaturalWidth: svgPreview.stageNaturalWidth,
          },
          pendingPreview: {
            activeThumbIndex: pendingPreview.activeThumbIndex,
            passed:
              pendingPreview.position === 2 &&
              pendingPreview.total === 11 &&
              pendingPreview.activeThumbIndex === 1,
          },
          restartRecovery: {
            passed: restartedHistory.historyAttachmentThumbCount >= 11,
            thumbCount: restartedHistory.historyAttachmentThumbCount,
          },
          themes: {
            highContrastPassed: highContrastThemePassed,
            lightPassed: lightThemePassed,
            passed: highContrastThemePassed && lightThemePassed,
          },
          zoom: {
            passed:
              typeof zoomedPreview.zoom === "number" &&
              zoomedPreview.zoom > 1,
            zoom: zoomedPreview.zoom,
          },
        },
        limitations: [
          "The 320px composer case uses the existing test-only root-width shim inside a real VS Code Webview because automated divider dragging is unreliable on macOS.",
          "Screenshots capture the full VS Code window via macOS screencapture; the copy-success state and codicon font load are checked here, but the native Save As dialog itself still needs manual acceptance.",
          "fullResolutionBytes is the arithmetic cost of decoding the same images at source resolution (11 x 4000 x 3000 x 4), not a second measured run: reintroducing the old data: URI pipeline to measure it would mean shipping the defect again.",
        ],
        screenshots,
        visualReview: {
          reviewedByAgent: false,
          status: "pending",
        },
      };
      await fs.writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`, "utf8");
      for (const [name, check] of Object.entries(report.checks)) {
        assert.equal(check.passed, true, `expected ${name} acceptance check to pass`);
      }
    } finally {
      if (blockedInlineDir) {
        await fs.rm(blockedInlineDir, { force: true, recursive: true });
      }
      await vscode.workspace
        .getConfiguration("workbench")
        .update(
          "colorTheme",
          originalColorTheme,
          vscode.ConfigurationTarget.Global,
        );
    }
  });
});
