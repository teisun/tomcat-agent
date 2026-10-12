import type { DraftAttachmentRef } from "../../shared/composerDraft";
import type { WebviewMessageSegment } from "./protocol";

export interface QueueContent {
  userMessageId: string;
  text: string;
  segments: WebviewMessageSegment[];
  attachments: DraftAttachmentRef[];
}
export interface QueueItem extends QueueContent { status: "queued" | "prompt" | "steering" }
interface Attempt { serial: number; kind: "prompt" | "steer"; stopAfterAck: boolean; pauseVersion: number }
interface SessionQueue {
  items: QueueItem[];
  paused: boolean;
  pauseVersion: number;
  editingId: string | null;
  inFlight?: Attempt;
  phase: "idle" | "starting" | "running";
}
interface Driver {
  conditions(sessionId: string): { busy: boolean; commandPending: boolean; enabled: boolean };
  send(sessionId: string, kind: "prompt" | "steer", content: QueueContent): Promise<{ success: boolean; error?: string | null }>;
  changed(sessionId: string): void;
  confirmed(sessionId: string, content: QueueContent, kind: "prompt" | "steer"): void;
  failed(sessionId: string, error: string): void;
  interrupt(sessionId: string): void;
}

/** Host-only volatile content queue. No transcript reconciliation or persistent delivery state. */
export class MessageQueue {
  private readonly sessions = new Map<string, SessionQueue>();
  private serial = 0;
  constructor(private readonly driver: Driver) {}
  private session(id: string): SessionQueue {
    let value = this.sessions.get(id);
    if (!value) { value = { items: [], paused: false, pauseVersion: 0, editingId: null, phase: "idle" }; this.sessions.set(id, value); }
    return value;
  }
  view(id: string): { items: readonly QueueItem[]; paused: boolean; editingId: string | null } {
    const s = this.session(id);
    return { items: s.items, paused: s.paused, editingId: s.editingId };
  }
  hasInFlight(id: string): boolean { return !!this.session(id).inFlight; }
  isRunning(id: string): boolean { return this.session(id).phase !== "idle"; }
  private busy(id: string): boolean { return this.isRunning(id) || this.driver.conditions(id).busy; }
  submit(id: string, content: QueueContent): { queued: boolean; completion: Promise<boolean> } {
    const s = this.session(id);
    if (this.busy(id) || s.inFlight) {
      if (this.driver.conditions(id).busy && s.phase === "idle") s.phase = "running";
      s.items.push({ ...content, status: "queued" });
      this.driver.changed(id);
      this.dispatch(id);
      return { queued: true, completion: Promise.resolve(true) };
    }
    return { queued: false, completion: this.send(id, "prompt", content, true) };
  }
  enqueue(id: string, content: QueueContent): void {
    const s = this.session(id);
    if (this.driver.conditions(id).busy && s.phase === "idle") s.phase = "running";
    s.items.push({ ...content, status: "queued" });
    this.driver.changed(id); this.dispatch(id);
  }
  sendNow(id: string, itemId: string): void {
    const s = this.session(id);
    if (s.inFlight) return;
    const index = s.items.findIndex(item => item.userMessageId === itemId && item.status === "queued");
    if (index < 0 || s.editingId === itemId || this.driver.conditions(id).commandPending) return;
    const [item] = s.items.splice(index, 1);
    s.items.unshift(item);
    const kind = this.busy(id) ? "steer" : "prompt";
    if (kind === "steer" && s.phase === "idle") s.phase = "running";
    void this.send(id, kind, item, true, item);
  }
  edit(id: string, itemId: string | null): void {
    const s = this.session(id);
    if (itemId && !s.items.some(item => item.userMessageId === itemId && item.status === "queued")) return;
    s.editingId = itemId; this.driver.changed(id); this.dispatch(id);
  }
  save(id: string, content: QueueContent): void {
    const s = this.session(id);
    const item = s.items.find(item => item.userMessageId === content.userMessageId && item.status === "queued");
    if (!item || s.editingId !== content.userMessageId) return;
    Object.assign(item, content); s.editingId = null;
    this.driver.changed(id); this.dispatch(id);
  }
  remove(id: string, itemId: string): void {
    const s = this.session(id);
    s.items = s.items.filter(item => item.userMessageId !== itemId || item.status !== "queued");
    if (s.editingId === itemId) s.editingId = null;
    this.driver.changed(id); this.dispatch(id);
  }
  start(id: string): void {
    const s = this.session(id), external = s.phase !== "starting";
    s.phase = "running";
    if (external && s.paused) { s.paused = false; this.driver.changed(id); }
  }
  stop(id: string): void {
    const s = this.session(id);
    s.paused = true; s.pauseVersion++;
    s.items = s.items.filter(item => item.status !== "steering");
    if (s.inFlight?.kind === "steer") s.inFlight = undefined;
    else if (s.inFlight) s.inFlight.stopAfterAck = true;
    this.driver.changed(id);
  }
  pause(id: string): void {
    const s = this.session(id); s.paused = true; s.pauseVersion++; s.editingId = null;
    this.driver.changed(id);
  }
  release(id: string): void {
    const s = this.session(id);
    this.pause(id);
    s.items = s.items.filter(item => item.status !== "steering");
    s.phase = "idle";
    s.inFlight = undefined;
    for (const item of s.items) if (item.status !== "queued") item.status = "queued";
    this.driver.changed(id);
  }
  cancelEditors(): void {
    for (const [id, s] of this.sessions) if (s.editingId) this.pause(id);
  }
  idle(id: string, outcome: "completed" | "interrupted" | "failed"): void {
    const s = this.session(id);
    // A repeated completed idle must not finish the next pending prompt.
    // Preparation can fail/cancel after ACK but before AgentLoop emits agent_start.
    if (s.phase === "idle" || (s.phase === "starting" && outcome === "completed")) return;
    s.phase = "idle";
    if (outcome === "interrupted") this.stop(id);
    else {
      for (const item of s.items) if (item.status === "steering") item.status = "queued";
      if (s.inFlight?.kind === "steer") s.inFlight = undefined;
      if (outcome === "failed") { s.paused = true; s.pauseVersion++; }
    }
    this.driver.changed(id); this.dispatch(id);
  }
  consumed(id: string, ids: readonly string[]): void {
    const s = this.session(id);
    for (const userMessageId of ids) {
      const item = s.items.find(item => item.userMessageId === userMessageId && item.status === "steering");
      if (item) this.driver.confirmed(id, item, "steer");
    }
    s.items = s.items.filter(item => !(item.status === "steering" && ids.includes(item.userMessageId)));
    this.driver.changed(id); this.dispatch(id);
  }
  forget(id: string): void { this.sessions.delete(id); }
  reset(preserve: ReadonlySet<string> = new Set()): void {
    for (const id of [...this.sessions.keys()]) {
      if (!preserve.has(id)) { this.sessions.delete(id); this.driver.changed(id); }
    }
  }
  dispatch(id: string): void {
    const s = this.session(id), conditions = this.driver.conditions(id), item = s.items[0];
    if (!conditions.enabled || conditions.commandPending || this.busy(id) || s.paused || s.inFlight || !item || item.status !== "queued" || s.editingId === item.userMessageId) return;
    void this.send(id, "prompt", item, false, item);
  }
  private async send(id: string, kind: "prompt" | "steer", content: QueueContent, explicit: boolean, item?: QueueItem): Promise<boolean> {
    const s = this.session(id);
    const attempt: Attempt = { serial: ++this.serial, kind, stopAfterAck: false, pauseVersion: s.pauseVersion };
    if (kind === "prompt" && s.phase === "idle") s.phase = "starting";
    s.inFlight = attempt;
    if (item) item.status = kind === "steer" ? "steering" : "prompt";
    this.driver.changed(id);
    let accepted = false;
    try {
      const response = await this.driver.send(id, kind, content);
      if (this.sessions.get(id) !== s || s.inFlight?.serial !== attempt.serial) return false;
      if (!response.success) {
        if (item) item.status = "queued";
        if (response.error !== "not_running") {
          s.paused = true; s.pauseVersion++;
          this.driver.failed(id, response.error ?? "Unable to send message");
        }
      } else {
        accepted = true;
        if (explicit && s.pauseVersion === attempt.pauseVersion) s.paused = false;
        if (kind === "prompt") {
          if (item) s.items = s.items.filter(value => value !== item);
          this.driver.confirmed(id, content, kind);
          if (attempt.stopAfterAck) this.driver.interrupt(id);
        }
      }
    } catch (error) {
      if (this.sessions.get(id) === s && s.inFlight?.serial === attempt.serial) {
        if (item) item.status = "queued";
        s.paused = true; s.pauseVersion++;
        this.driver.failed(id, error instanceof Error ? error.message : String(error));
      }
    } finally {
      if (this.sessions.get(id) === s && s.inFlight?.serial === attempt.serial) {
        if (kind === "prompt" && !accepted && s.phase === "starting") s.phase = "idle";
        s.inFlight = undefined; this.driver.changed(id); this.dispatch(id);
      }
    }
    return accepted;
  }
}
