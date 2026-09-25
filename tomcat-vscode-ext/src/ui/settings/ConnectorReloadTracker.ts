import {
  CONNECTOR_PROTOCOL_MISMATCH,
  parseConnectorReloadReceipt,
  type ConnectorView,
} from "../../shared/connectorsProtocol";
import type { SettingsConnectorReloadReceipt } from "../../shared/settingsProtocol";

export interface ReloadObservation {
  receipt: SettingsConnectorReloadReceipt;
  afterRead: number;
  deadline?: number;
}

/** Host-owned click bookkeeping only. Connection facts always come from Serve. */
export class ConnectorReloadTracker {
  private readonly entries = new Map<string, ReloadObservation>();

  begin(configKey: string, requestId: string): ReloadObservation | undefined {
    const previous = this.entries.get(configKey);
    if (previous && this.active(previous)) return undefined;
    const entry: ReloadObservation = { receipt: { configKey, requestId, phase: "pending" }, afterRead: 0 };
    this.entries.set(configKey, entry);
    return entry;
  }

  active(entry: ReloadObservation): boolean {
    return this.entries.get(entry.receipt.configKey) === entry
      && (entry.receipt.phase === "pending" || entry.receipt.phase === "accepted");
  }

  get busy(): boolean { return [...this.entries.values()].some((entry) => this.active(entry)); }

  snapshot(): Record<string, SettingsConnectorReloadReceipt> {
    return Object.fromEntries([...this.entries].map(([key, entry]) => [key, { ...entry.receipt }]));
  }

  clear(): void { this.entries.clear(); }

  accept(entry: ReloadObservation, payload: unknown, afterRead: number, now: number): void {
    if (!this.active(entry) || entry.receipt.phase !== "pending") return;
    const receipt = parseConnectorReloadReceipt(payload, entry.receipt.configKey);
    const deadline = now + receipt.recoveryTimeoutMs + 10_000;
    if (!Number.isSafeInteger(deadline)) throw new Error(CONNECTOR_PROTOCOL_MISMATCH);
    entry.afterRead = afterRead;
    entry.deadline = deadline;
    entry.receipt = { ...entry.receipt, phase: "accepted", generation: receipt.generation, message: "Reconnection accepted." };
  }

  finish(entry: ReloadObservation, phase: "failed" | "unknown", message: string, reason?: SettingsConnectorReloadReceipt["reason"]): void {
    if (!this.active(entry)) return;
    entry.receipt = { ...entry.receipt, phase, message, reason };
  }

  disconnected(): void {
    for (const entry of this.entries.values()) {
      this.finish(entry, "unknown", "Unable to confirm reconnection. Connection lost.", "connection-lost");
    }
  }

  expire(now: number): boolean {
    let changed = false;
    for (const entry of this.entries.values()) {
      if (this.active(entry) && entry.deadline !== undefined && now >= entry.deadline) {
        this.finish(entry, "unknown", "Unable to confirm reconnection before the observation deadline.", "timeout");
        changed = true;
      }
    }
    return changed;
  }

  observe(connectors: ConnectorView[], read: number): void {
    for (const entry of this.entries.values()) {
      if (entry.receipt.phase !== "accepted" || read <= entry.afterRead) continue;
      const connector = connectors.find((candidate) => candidate.configKey === entry.receipt.configKey);
      if (!connector) {
        this.finish(entry, "failed", "Connector was removed during reconnection.", "removed");
        continue;
      }
      if (connector.compatibilityError || connector.generation === undefined) {
        this.finish(entry, "unknown", CONNECTOR_PROTOCOL_MISMATCH, "incompatible");
        continue;
      }
      const expected = entry.receipt.generation!;
      if (connector.generation !== expected) {
        // An older read cannot settle a new click. A newer recovery supersedes it.
        if (connector.generation.length > expected.length
          || (connector.generation.length === expected.length && connector.generation > expected)) {
          this.finish(entry, "failed", "Reconnection was superseded by a newer recovery.", "superseded");
        }
        continue;
      }
      if (connector.overridden) {
        this.finish(entry, "failed", "Connector is overridden by workspace configuration.", "superseded");
      } else if (connector.state === "connected") {
        entry.receipt = { ...entry.receipt, phase: "succeeded", message: "Connector reconnected." };
      } else if (connector.state !== "pending" && connector.state !== "connecting") {
        const nextStep = connector.state === "needs_authorization" ? "Login is required."
          : connector.state === "awaiting_project_trust" ? "Trust this project to connect its services."
            : "Connector did not reconnect.";
        this.finish(entry, "failed", connector.error ?? nextStep, "rejected");
      }
    }
  }
}
