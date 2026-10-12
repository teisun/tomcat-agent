import { Buffer } from "node:buffer";
import { performance } from "node:perf_hooks";
import process from "node:process";

import { render } from "@testing-library/react";
import { JSDOM } from "jsdom";
import React, { Profiler, memo } from "react";

import { applySessionPatchFrame } from "../src/statePatch";
import { mergeSessionViewSnapshot, reconcileStateSnapshot } from "../src/stateReconcile";
import type {
  HostToWebviewFrame,
  WebviewMessageBlock,
  WebviewSessionPatchOp,
  WebviewSessionSnapshot,
  WebviewSessionTab,
  WebviewStateSnapshot,
} from "../src/types";

interface Options {
  chunkSize: number;
  deltas: number;
  itemsPerSession: number;
  json: boolean;
  sessions: number;
}

interface NumericSummary {
  average: number;
  max: number;
  min: number;
  p50: number;
  p95: number;
}

interface RenderSummary {
  mountActualDurationMs: number;
  mountCommits: number;
  mountRowRenders: number;
  updateActualDurationMs: number;
  updateCommits: number;
  updateRowRenders: number;
}

interface SessionViewFrameContent {
  sessionId: string;
  tab: WebviewSessionTab;
  view: WebviewSessionSnapshot;
}

interface SessionPatchFrameContent {
  ops: WebviewSessionPatchOp[];
  seq: number;
  sessionId: string;
}

const STREAM_SESSION_ID = "s1";
const STREAM_ITEM_ID = `${STREAM_SESSION_ID}-assistant-tail`;
const DEFAULTS: Options = {
  chunkSize: 6,
  deltas: 200,
  itemsPerSession: 500,
  json: false,
  sessions: 3,
};

function installDomGlobals() {
  const dom = new JSDOM("<!doctype html><html><body></body></html>", {
    url: "http://localhost/",
  });
  const { window } = dom;
  const globals: Record<string, unknown> = {
    document: window.document,
    HTMLElement: window.HTMLElement,
    IS_REACT_ACT_ENVIRONMENT: true,
    Node: window.Node,
    Text: window.Text,
    documentElement: window.document.documentElement,
    navigator: window.navigator,
    self: window,
    window,
  };
  for (const [key, value] of Object.entries(globals)) {
    Object.defineProperty(globalThis, key, {
      configurable: true,
      value,
      writable: true,
    });
  }
  Object.defineProperty(globalThis, "getComputedStyle", {
    configurable: true,
    value: window.getComputedStyle.bind(window),
    writable: true,
  });
  Object.defineProperty(globalThis, "requestAnimationFrame", {
    configurable: true,
    value: (callback: FrameRequestCallback) =>
      setTimeout(() => callback(performance.now()), 0),
    writable: true,
  });
  Object.defineProperty(globalThis, "cancelAnimationFrame", {
    configurable: true,
    value: (handle: ReturnType<typeof setTimeout>) => clearTimeout(handle),
    writable: true,
  });
  if (typeof window.matchMedia !== "function") {
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: () => ({
        addEventListener() {},
        addListener() {},
        dispatchEvent() {
          return false;
        },
        matches: false,
        media: "",
        onchange: null,
        removeEventListener() {},
        removeListener() {},
      }),
    });
  }
}

function parsePositiveInt(flag: string, raw: string | undefined, fallback: number): number {
  if (!raw) {
    return fallback;
  }
  const value = Number.parseInt(raw, 10);
  if (!Number.isFinite(value) || value <= 0) {
    throw new Error(`${flag} must be a positive integer, got ${raw}`);
  }
  return value;
}

function parseOptions(argv: readonly string[]): Options {
  const options: Options = { ...DEFAULTS };
  for (const argument of argv) {
    if (argument === "--json") {
      options.json = true;
      continue;
    }
    const [flag, rawValue] = argument.split("=", 2);
    switch (flag) {
      case "--deltas":
        options.deltas = parsePositiveInt(flag, rawValue, options.deltas);
        break;
      case "--items":
        options.itemsPerSession = parsePositiveInt(flag, rawValue, options.itemsPerSession);
        break;
      case "--sessions":
        options.sessions = parsePositiveInt(flag, rawValue, options.sessions);
        break;
      case "--chunk-size":
        options.chunkSize = parsePositiveInt(flag, rawValue, options.chunkSize);
        break;
      default:
        throw new Error(
          `Unknown argument ${argument}. Supported: --deltas=, --items=, --sessions=, --chunk-size=, --json`,
        );
    }
  }
  return options;
}

function cloneJson<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function makeText(seed: string, minLength: number): string {
  const unit = `${seed} lorem ipsum dolor sit amet consectetur adipiscing elit. `;
  let result = "";
  while (result.length < minLength) {
    result += unit;
  }
  return result.slice(0, minLength);
}

function makeDeltaChunk(index: number, chunkSize: number): string {
  const body = `d${index.toString(36)}`.padEnd(Math.max(chunkSize - 1, 1), "x");
  return ` ${body}`.slice(0, chunkSize);
}

function buildTimeline(
  sessionId: string,
  itemsPerSession: number,
  streamingTailText: string,
): WebviewMessageBlock[] {
  const timeline: WebviewMessageBlock[] = [];
  for (let index = 0; index < itemsPerSession; index += 1) {
    const isTail = index === itemsPerSession - 1;
    const isAssistant = isTail || index % 2 === 1;
    const id = isTail ? `${sessionId}-assistant-tail` : `${sessionId}-m-${index}`;
    timeline.push({
      assistantMessageId: isAssistant ? id : undefined,
      id,
      kind: isAssistant ? "assistant" : "user",
      text: isTail ? streamingTailText : makeText(`${sessionId}-${index}`, 140),
      type: "message",
    });
  }
  if (sessionId === STREAM_SESSION_ID) {
    timeline[timeline.length - 1] = {
      ...timeline[timeline.length - 1],
      assistantMessageId: STREAM_ITEM_ID,
      id: STREAM_ITEM_ID,
    };
  }
  return timeline;
}

function buildBaseState(options: Options): WebviewStateSnapshot {
  const sessions: WebviewSessionTab[] = [];
  const sessionViews: Record<string, WebviewSessionSnapshot> = {};
  for (let index = 0; index < options.sessions; index += 1) {
    const sessionId = `s${index + 1}`;
    sessions.push({
      busy: sessionId === STREAM_SESSION_ID,
      isCurrent: sessionId === STREAM_SESSION_ID,
      ownedByThisFrontend: true,
      sessionId,
      title: `Session ${index + 1}`,
      updatedAt: 1,
    });
    sessionViews[sessionId] = {
      busy: sessionId === STREAM_SESSION_ID,
      checkpoints: [],
      contextRatio: null,
      hasMoreHistory: false,
      historyLoading: false,
      model: "gpt-5.4",
      ownedByThisFrontend: true,
      pendingAttachments: [],
      planFile: null,
      planId: null,
      planState: "chat",
      planTodos: [],
      sessionId,
      sessionTodos: [],
      thinkingLevel: "high",
      timeline: buildTimeline(
        sessionId,
        options.itemsPerSession,
        makeText(`${sessionId}-tail`, 160),
      ),
    };
  }
  return {
    activeSessionId: STREAM_SESSION_ID,
    availableModelCapabilities: {},
    availableModelReasoningLevels: {},
    availableModels: ["gpt-5.4"],
    buildModel: "",
    modelAdminSupported: false,
    ready: true,
    sessionViews,
    sessions,
  };
}

function summarize(samples: readonly number[]): NumericSummary {
  if (samples.length === 0) {
    return { average: 0, max: 0, min: 0, p50: 0, p95: 0 };
  }
  const sorted = [...samples].sort((left, right) => left - right);
  const percentile = (ratio: number) => sorted[Math.min(sorted.length - 1, Math.floor(ratio * (sorted.length - 1)))];
  const total = samples.reduce((sum, value) => sum + value, 0);
  return {
    average: total / samples.length,
    max: sorted[sorted.length - 1],
    min: sorted[0],
    p50: percentile(0.5),
    p95: percentile(0.95),
  };
}

function formatMs(value: number): string {
  return `${value.toFixed(3)} ms`;
}

function formatBytes(value: number): string {
  return `${Math.round(value).toLocaleString("en-US")} B`;
}

function frameBytes(frame: HostToWebviewFrame): number {
  return Buffer.byteLength(JSON.stringify(frame), "utf8");
}

function assertMessageTail(state: WebviewStateSnapshot): WebviewMessageBlock {
  const session = state.sessionViews[STREAM_SESSION_ID];
  if (!session) {
    throw new Error(`Missing session ${STREAM_SESSION_ID}`);
  }
  const tail = session.timeline[session.timeline.length - 1];
  if (!tail || tail.type !== "message" || tail.kind !== "assistant" || tail.id !== STREAM_ITEM_ID) {
    throw new Error("Synthetic fixture tail is not the expected streaming assistant message");
  }
  return tail;
}

function buildFrames(options: Options) {
  const authoritative = buildBaseState(options);
  const baseState = cloneJson(authoritative);
  const cloneMs: number[] = [];
  const fullStateBytes: number[] = [];
  const sessionViewBytes: number[] = [];
  const patchBytes: number[] = [];
  const fullFrames: Array<Extract<HostToWebviewFrame, { channel: "state" }>> = [];
  const sessionViewFrames: Array<Extract<HostToWebviewFrame, { channel: "sessionView" }>> = [];
  const patchFrames: Array<Extract<HostToWebviewFrame, { channel: "sessionPatch" }>> = [];

  for (let index = 0; index < options.deltas; index += 1) {
    const chunk = makeDeltaChunk(index + 1, options.chunkSize);
    const tail = assertMessageTail(authoritative);
    tail.text = `${tail.text}${chunk}`;

    const cloneStart = performance.now();
    const snapshot = cloneJson(authoritative);
    cloneMs.push(performance.now() - cloneStart);

    const stateFrame: Extract<HostToWebviewFrame, { channel: "state" }> = {
      channel: "state",
      content: snapshot,
      messageId: `state-${index + 1}`,
    };
    fullFrames.push(stateFrame);
    fullStateBytes.push(frameBytes(stateFrame));

    const sessionViewFrame: Extract<HostToWebviewFrame, { channel: "sessionView" }> = {
      channel: "sessionView",
      content: {
        sessionId: STREAM_SESSION_ID,
        tab: snapshot.sessions.find((entry) => entry.sessionId === STREAM_SESSION_ID)!,
        view: snapshot.sessionViews[STREAM_SESSION_ID],
      },
      messageId: `session-view-${index + 1}`,
    };
    sessionViewFrames.push(sessionViewFrame);
    sessionViewBytes.push(frameBytes(sessionViewFrame));

    const patchFrame: Extract<HostToWebviewFrame, { channel: "sessionPatch" }> = {
      channel: "sessionPatch",
      content: {
        ops: [{ id: STREAM_ITEM_ID, text: chunk, type: "appendText" }],
        seq: index + 1,
        sessionId: STREAM_SESSION_ID,
      },
      messageId: `session-patch-${index + 1}`,
    };
    patchFrames.push(patchFrame);
    patchBytes.push(frameBytes(patchFrame));
  }

  return {
    baseState,
    cloneMs,
    fullFrames,
    fullStateBytes,
    patchBytes,
    patchFrames,
    sessionViewBytes,
    sessionViewFrames,
  };
}

function measureGuiMerges(run: ReturnType<typeof buildFrames>) {
  const reconcileMs: number[] = [];
  const sessionViewMs: number[] = [];
  const patchMs: number[] = [];

  let reconciled = run.baseState;
  let mergedSessionView = run.baseState;
  let patched = run.baseState;

  for (let index = 0; index < run.fullFrames.length; index += 1) {
    const fullFrame = run.fullFrames[index];
    const sessionViewFrame = run.sessionViewFrames[index];
    const patchFrame = run.patchFrames[index];

    const reconcileStart = performance.now();
    reconciled = reconcileStateSnapshot(reconciled, fullFrame.content);
    reconcileMs.push(performance.now() - reconcileStart);

    const sessionViewStart = performance.now();
    mergedSessionView = mergeSessionViewSnapshot(
      mergedSessionView,
      sessionViewFrame.content as SessionViewFrameContent,
    );
    sessionViewMs.push(performance.now() - sessionViewStart);

    const patchStart = performance.now();
    const patchResult = applySessionPatchFrame(patched, patchFrame.content as SessionPatchFrameContent);
    patchMs.push(performance.now() - patchStart);
    if (!patchResult.ok) {
      throw new Error(`Patch benchmark failed at delta ${index + 1}: ${patchResult.error}`);
    }
    patched = patchResult.state;
  }

  const reconciledJson = JSON.stringify(reconciled);
  const mergedJson = JSON.stringify(mergedSessionView);
  const patchedJson = JSON.stringify(patched);
  if (reconciledJson !== mergedJson) {
    throw new Error("sessionView merge drifted from full-state reconcile");
  }
  if (reconciledJson !== patchedJson) {
    throw new Error("sessionPatch apply drifted from full-state reconcile");
  }

  return {
    patchMs,
    reconcileMs,
    sessionViewMs,
  };
}

type BenchItem = WebviewMessageBlock;

let activeRowCounter = 0;

const BenchRow = memo(
  function BenchRow({ item }: { item: BenchItem }) {
    activeRowCounter += 1;
    return <div>{item.text}</div>;
  },
  (previous, next) => previous.item === next.item,
);

function BenchTimeline({ timeline }: { timeline: readonly BenchItem[] }) {
  return (
    <div>
      {timeline.map((item) => (
        <BenchRow item={item} key={item.id} />
      ))}
    </div>
  );
}

function toBenchItems(state: WebviewStateSnapshot): readonly BenchItem[] {
  return state.sessionViews[STREAM_SESSION_ID].timeline.map((item) => {
    if (item.type !== "message") {
      throw new Error("Synthetic benchmark expects a pure message timeline");
    }
    return item;
  });
}

function measureRenderSequence(states: readonly WebviewStateSnapshot[]): RenderSummary {
  activeRowCounter = 0;
  const summary: RenderSummary = {
    mountActualDurationMs: 0,
    mountCommits: 0,
    mountRowRenders: 0,
    updateActualDurationMs: 0,
    updateCommits: 0,
    updateRowRenders: 0,
  };
  const { rerender, unmount } = render(
    <Profiler
      id="bench-timeline"
      onRender={(_id, phase, actualDuration) => {
        if (phase === "mount") {
          summary.mountActualDurationMs += actualDuration;
          summary.mountCommits += 1;
          return;
        }
        summary.updateActualDurationMs += actualDuration;
        summary.updateCommits += 1;
      }}
    >
      <BenchTimeline timeline={toBenchItems(states[0])} />
    </Profiler>,
  );
  summary.mountRowRenders = activeRowCounter;
  activeRowCounter = 0;

  for (let index = 1; index < states.length; index += 1) {
    rerender(
      <Profiler
        id="bench-timeline"
        onRender={(_id, phase, actualDuration) => {
          if (phase === "mount") {
            summary.mountActualDurationMs += actualDuration;
            summary.mountCommits += 1;
            return;
          }
          summary.updateActualDurationMs += actualDuration;
          summary.updateCommits += 1;
        }}
      >
        <BenchTimeline timeline={toBenchItems(states[index])} />
      </Profiler>,
    );
    summary.updateRowRenders += activeRowCounter;
    activeRowCounter = 0;
  }

  unmount();
  return summary;
}

function buildRenderStates(run: ReturnType<typeof buildFrames>) {
  const fullNoReuse: WebviewStateSnapshot[] = [cloneJson(run.baseState)];
  const fullReconcile: WebviewStateSnapshot[] = [run.baseState];
  const patch: WebviewStateSnapshot[] = [run.baseState];

  let reconciled = run.baseState;
  let patched = run.baseState;

  for (let index = 0; index < run.fullFrames.length; index += 1) {
    const fullSnapshot = run.fullFrames[index].content;
    fullNoReuse.push(fullSnapshot);

    reconciled = reconcileStateSnapshot(reconciled, fullSnapshot);
    fullReconcile.push(reconciled);

    const patchResult = applySessionPatchFrame(patched, run.patchFrames[index].content as SessionPatchFrameContent);
    if (!patchResult.ok) {
      throw new Error(`Patch render benchmark failed at delta ${index + 1}: ${patchResult.error}`);
    }
    patched = patchResult.state;
    patch.push(patched);
  }

  return { fullNoReuse, fullReconcile, patch };
}

function buildReport(options: Options) {
  const run = buildFrames(options);
  const mergeMetrics = measureGuiMerges(run);
  const renderStates = buildRenderStates(run);
  const renderMetrics = {
    fullNoReuse: measureRenderSequence(renderStates.fullNoReuse),
    fullReconcile: measureRenderSequence(renderStates.fullReconcile),
    patch: measureRenderSequence(renderStates.patch),
  };

  return {
    config: {
      chunkSize: options.chunkSize,
      deltas: options.deltas,
      itemsPerSession: options.itemsPerSession,
      sessions: options.sessions,
    },
    guiMerge: {
      patch: summarize(mergeMetrics.patchMs),
      reconcileFullState: summarize(mergeMetrics.reconcileMs),
      sessionView: summarize(mergeMetrics.sessionViewMs),
    },
    hostBridge: {
      fullStateCloneMs: summarize(run.cloneMs),
      fullStateFrameBytes: summarize(run.fullStateBytes),
      sessionPatchFrameBytes: summarize(run.patchBytes),
      sessionViewFrameBytes: summarize(run.sessionViewBytes),
    },
    renderHarness: {
      comparator: "prev.item === next.item",
      fullNoReuse: renderMetrics.fullNoReuse,
      fullReconcile: renderMetrics.fullReconcile,
      patch: renderMetrics.patch,
      rowsPerDeltaExpectedHotPath: 1,
    },
  };
}

function printNumericSummary(
  label: string,
  summary: NumericSummary,
  formatter: (value: number) => string,
) {
  console.log(
    `${label}: avg ${formatter(summary.average)} | p50 ${formatter(summary.p50)} | p95 ${formatter(summary.p95)} | min ${formatter(summary.min)} | max ${formatter(summary.max)}`,
  );
}

function printRenderSummary(label: string, summary: RenderSummary, deltas: number) {
  const rowsPerDelta = deltas === 0 ? 0 : summary.updateRowRenders / deltas;
  console.log(
    `${label}: update row renders ${summary.updateRowRenders.toLocaleString("en-US")} (${rowsPerDelta.toFixed(1)}/delta) | update commits ${summary.updateCommits} | update duration ${formatMs(summary.updateActualDurationMs)}`,
  );
}

function main() {
  installDomGlobals();
  const options = parseOptions(process.argv.slice(2));
  const report = buildReport(options);

  if (options.json) {
    console.log(JSON.stringify(report, null, 2));
    return;
  }

  console.log("Webview Bridge Benchmark");
  console.log(
    `fixture: ${report.config.sessions} sessions x ${report.config.itemsPerSession} items/session x ${report.config.deltas} streaming deltas`,
  );
  console.log("");
  console.log("Host bridge");
  printNumericSummary("  full state clone", report.hostBridge.fullStateCloneMs, formatMs);
  printNumericSummary("  full state frame", report.hostBridge.fullStateFrameBytes, formatBytes);
  printNumericSummary("  sessionView frame", report.hostBridge.sessionViewFrameBytes, formatBytes);
  printNumericSummary("  sessionPatch frame", report.hostBridge.sessionPatchFrameBytes, formatBytes);
  console.log("");
  console.log("GUI merge");
  printNumericSummary("  reconcileStateSnapshot(full state)", report.guiMerge.reconcileFullState, formatMs);
  printNumericSummary("  mergeSessionViewSnapshot", report.guiMerge.sessionView, formatMs);
  printNumericSummary("  applySessionPatchFrame", report.guiMerge.patch, formatMs);
  console.log("");
  console.log("Memo row harness");
  console.log(`  comparator: ${report.renderHarness.comparator}`);
  printRenderSummary("  full state without reuse", report.renderHarness.fullNoReuse, report.config.deltas);
  printRenderSummary("  full state + reconcile", report.renderHarness.fullReconcile, report.config.deltas);
  printRenderSummary("  sessionPatch hot path", report.renderHarness.patch, report.config.deltas);
  console.log("");
  console.log("Key ratios");
  console.log(
    `  sessionPatch payload vs full state: ${(report.hostBridge.fullStateFrameBytes.average / report.hostBridge.sessionPatchFrameBytes.average).toFixed(1)}x smaller`,
  );
  console.log(
    `  sessionView payload vs full state: ${(report.hostBridge.fullStateFrameBytes.average / report.hostBridge.sessionViewFrameBytes.average).toFixed(1)}x smaller`,
  );
  console.log(
    `  sessionPatch merge vs full reconcile: ${(report.guiMerge.reconcileFullState.average / report.guiMerge.patch.average).toFixed(1)}x faster`,
  );
  console.log(
    `  reconcile/patch row renders vs no reuse: ${(report.renderHarness.fullNoReuse.updateRowRenders / report.renderHarness.patch.updateRowRenders).toFixed(1)}x fewer`,
  );
}

main();
