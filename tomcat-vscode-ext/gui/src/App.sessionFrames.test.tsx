import { act, render, screen } from "@testing-library/react";
import { fireEvent } from "@testing-library/react";
import { beforeAll, describe, expect, it, vi } from "vitest";

import { App } from "./App";
import type { HostToWebviewFrame, VsCodeApiLike, WebviewStateSnapshot } from "./types";

beforeAll(() => {
  Object.defineProperty(Range.prototype,"getClientRects",{configurable:true,value:()=>[]});
  Object.defineProperty(Range.prototype,"getBoundingClientRect",{configurable:true,value:()=>({left:0,right:0,top:0,bottom:0,width:0,height:0})});
});

function mount() {
  const postMessage = vi.fn();
  const vscodeApi: VsCodeApiLike = {
    postMessage,
    setState: vi.fn(),
  };
  render(<App vscodeApi={vscodeApi} />);
  return { postMessage };
}

async function emitFrame(frame: HostToWebviewFrame) {
  await act(async () => {
    window.dispatchEvent(new MessageEvent("message", { data: frame }));
  });
}

function baseState(): WebviewStateSnapshot {
  return {
    activeSessionId: "s1",
    availableModelCapabilities: {},
    availableModelReasoningLevels: {},
    availableModels: ["gpt-5.4"],
    modelAdminSupported: false,
    ready: true,
    sessions: [
      {
        busy: false,
        isCurrent: true,
        ownedByThisFrontend: true,
        sessionId: "s1",
        title: "Session 1",
        updatedAt: 1,
      },
    ],
    sessionViews: {
      s1: {
        activePlan: null,
        agentMode: "chat",
        busy: false,
        checkpoints: [],
        contextRatio: null,
        hasMoreHistory: false,
        historyLoading: false,
        model: "gpt-5.4",
        ownedByThisFrontend: true,
        pendingAttachments: [],
        planTodos: [],
        sessionId: "s1",
        sessionTodos: [],
        thinkingLevel: "high",
        timeline: [
          {
            assistantMessageId: "assistant-1",
            id: "assistant-1",
            kind: "assistant",
            text: "hel",
            type: "message",
          },
        ],
      },
    },
  };
}

describe("App session frames", () => {
  it("rebinds transcript resize after reconnect before append-only patches", async () => {
    const observers: Array<{ callback: ResizeObserverCallback; nodes: Element[]; disconnect: ReturnType<typeof vi.fn> }> = [];
    vi.stubGlobal("ResizeObserver", class {
      nodes: Element[] = [];
      disconnect = vi.fn();
      readonly callback: ResizeObserverCallback;
      constructor(callback: ResizeObserverCallback) {
        this.callback = callback;
        observers.push(this);
      }
      observe(node: Element) { this.nodes.push(node); }
      unobserve() {}
    });
    try {
      mount();
      const snapshot = baseState();
      await emitFrame({ channel: "state", content: snapshot, messageId: "initial" });
      const original = screen.getByTestId("transcript");
      const oldObserver = observers.find((observer) => observer.nodes.includes(original));
      expect(oldObserver).toBeDefined();
      await emitFrame({ channel: "state", content: { ...snapshot, ready: false }, messageId: "disconnect" });
      expect(screen.queryByTestId("transcript")).toBeNull();
      expect(oldObserver!.disconnect).toHaveBeenCalledOnce();
      await emitFrame({ channel: "state", content: snapshot, messageId: "reconnect" });
      const current = screen.getByTestId("transcript");
      expect(current).not.toBe(original);
      const observer = observers.find((entry) => entry.nodes.includes(current));
      expect(observer).toBeDefined();
      expect(observer).not.toBe(oldObserver);
      await emitFrame({ channel: "sessionPatch", messageId: "append", content: {
        sessionId: "s1", seq: 1, ops: [{ type: "appendText", id: "assistant-1", text: "lo" }],
      } });
      expect(screen.getByText("hello")).toBeTruthy();
      const root = screen.getByTestId("stream-container");
      Object.defineProperty(root, "clientHeight", { configurable: true, value: 100 });
      Object.defineProperty(root, "scrollHeight", { configurable: true, value: 500 });
      act(() => observer!.callback([], {} as ResizeObserver));
      expect(root.scrollTop).toBe(400);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("allows Stop after switching between two busy sessions without remounting the editor", async () => {
    const { postMessage } = mount();
    const initial = baseState();
    initial.sessions[0].busy = true;
    initial.sessionViews.s1.busy = true;
    initial.sessionViews.s2 = { ...initial.sessionViews.s1, sessionId: "s2" };
    initial.sessions.push({ ...initial.sessions[0], sessionId: "s2", isCurrent: false, title: "Session 2" });
    await emitFrame({ channel: "state", content: initial, messageId: "busy-a" });
    const editor = screen.getByTestId("composer-input");
    fireEvent.click(screen.getByTestId("stop-button"));
    expect(screen.getByTestId("stop-button")).toHaveProperty("disabled", true);
    await emitFrame({ channel: "state", messageId: "busy-b", content: {
      ...initial, activeSessionId: "s2", sessions: initial.sessions.map(tab => ({ ...tab, isCurrent: tab.sessionId === "s2" })),
    } });
    expect(screen.getByTestId("composer-input")).toBe(editor);
    expect(screen.getByTestId("stop-button")).toHaveProperty("disabled", false);
    fireEvent.click(screen.getByTestId("stop-button"));
    expect(postMessage.mock.calls.filter(([message]) => message.type === "interrupt").map(([message]) => message.data.sessionId)).toEqual(["s1", "s2"]);
  });

  it.each([true,false])("keeps repeated additions but not duplicate deliveries (event first=%s)", async (eventFirst) => {
    const errors = vi.spyOn(console, "error");
    try {
      mount();
      const initial = baseState(); initial.sessionViews.s1.timeline = [];
      await emitFrame({channel:"state",content:initial,messageId:"init"});
      const reference = {type:"reference" as const,kind:"file" as const,label:"app.ts",path:"src/app.ts",occurrenceId:"first"};
      const event = {channel:"event" as const,content:{type:"insertReference" as const,sessionId:"s1",reference},messageId:"event"};
      const next = baseState(); next.sessionViews.s1.timeline = []; next.sessionViews.s1.composerDraft = {text:"",segments:[reference]};
      const snapshot = {channel:"state" as const,content:next,messageId:"snapshot"};
      await emitFrame(eventFirst ? event : snapshot);
      await emitFrame(eventFirst ? snapshot : event);
      expect(screen.getAllByTestId("composer-reference-chip")).toHaveLength(1);
      await emitFrame({...event,messageId:"second",content:{...event.content,reference:{...reference,occurrenceId:"second"}}});
      expect(screen.getAllByTestId("composer-reference-chip")).toHaveLength(2);
      await act(async () => { fireEvent.click(screen.getAllByTestId("composer-reference-chip")[0].querySelector('button')!); });
      await emitFrame({...event,messageId:"late"});
      await emitFrame({...snapshot,messageId:"late-snapshot"});
      expect(screen.getAllByTestId("composer-reference-chip")).toHaveLength(1);
      expect(errors).not.toHaveBeenCalled();
    } finally {
      errors.mockRestore();
    }
  });
  it("applies sessionPatch frames to the active transcript", async () => {
    mount();
    await emitFrame({
      channel: "state",
      content: baseState(),
      messageId: "state-1",
    });

    expect(screen.getByText("hel")).toBeTruthy();

    await emitFrame({
      channel: "sessionPatch",
      content: {
        ops: [{ id: "assistant-1", text: "lo", type: "appendText" }],
        seq: 1,
        sessionId: "s1",
      },
      messageId: "patch-1",
    });

    expect(screen.getByText("hello")).toBeTruthy();
  });

  it("requests a session resync when patch seqs skip ahead", async () => {
    const { postMessage } = mount();
    await emitFrame({
      channel: "state",
      content: baseState(),
      messageId: "state-1",
    });

    await emitFrame({
      channel: "sessionPatch",
      content: {
        ops: [{ id: "assistant-1", text: "lo", type: "appendText" }],
        seq: 1,
        sessionId: "s1",
      },
      messageId: "patch-1",
    });
    await emitFrame({
      channel: "sessionPatch",
      content: {
        ops: [{ id: "assistant-1", text: " world", type: "appendText" }],
        seq: 3,
        sessionId: "s1",
      },
      messageId: "patch-gap",
    });

    expect(postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        data: { sessionId: "s1" },
        type: "resyncSessionView",
      }),
    );
  });

  it("clears resync-pending state when a sessionView arrives", async () => {
    const { postMessage } = mount();
    await emitFrame({
      channel: "state",
      content: baseState(),
      messageId: "state-1",
    });

    await emitFrame({
      channel: "sessionPatch",
      content: {
        ops: [{ id: "assistant-1", text: "lo", type: "appendText" }],
        seq: 1,
        sessionId: "s1",
      },
      messageId: "patch-1",
    });
    await emitFrame({
      channel: "sessionPatch",
      content: {
        ops: [{ id: "assistant-1", text: " world", type: "appendText" }],
        seq: 3,
        sessionId: "s1",
      },
      messageId: "patch-gap",
    });

    await emitFrame({
      channel: "sessionView",
      content: {
        sessionId: "s1",
        tab: {
          busy: false,
          isCurrent: true,
          ownedByThisFrontend: true,
          sessionId: "s1",
          title: "Session 1",
          updatedAt: 2,
        },
        view: {
          ...baseState().sessionViews.s1,
          timeline: [
            {
              assistantMessageId: "assistant-1",
              id: "assistant-1",
              kind: "assistant",
              text: "hello world",
              type: "message",
            },
          ],
        },
      },
      messageId: "session-view-1",
    });

    await emitFrame({
      channel: "sessionPatch",
      content: {
        ops: [{ id: "assistant-1", text: "!", type: "appendText" }],
        seq: 4,
        sessionId: "s1",
      },
      messageId: "patch-after-resync",
    });

    expect(screen.getByText("hello world!")).toBeTruthy();
    expect(
      postMessage.mock.calls.filter(
        ([message]) => (message as { type?: string }).type === "resyncSessionView",
      ),
    ).toHaveLength(1);
  });
});
