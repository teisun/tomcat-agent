import type { FrontendOwnerKind } from "./protocol";

export interface SessionOwnershipRecord {
  claimedAt: number;
  sessionId: string;
  owner: FrontendOwnerKind;
}

export type ClaimOwnershipResult =
  | {
      ok: true;
      record: SessionOwnershipRecord;
    }
  | {
      ok: false;
      record: SessionOwnershipRecord;
    };

export class SessionOwnershipTracker {
  private readonly owners = new Map<string, SessionOwnershipRecord>();

  claim(
    sessionId: string,
    owner: FrontendOwnerKind,
  ): ClaimOwnershipResult {
    const existing = this.owners.get(sessionId);
    if (existing && existing.owner !== owner) {
      return {
        ok: false,
        record: existing,
      };
    }
    const record = existing ?? {
      claimedAt: Date.now(),
      owner,
      sessionId,
    };
    this.owners.set(sessionId, record);
    return {
      ok: true,
      record,
    };
  }

  ownerOf(sessionId: string): SessionOwnershipRecord | undefined {
    return this.owners.get(sessionId);
  }

  release(sessionId: string, owner?: FrontendOwnerKind): boolean {
    const existing = this.owners.get(sessionId);
    if (!existing) {
      return false;
    }
    if (owner && existing.owner !== owner) {
      return false;
    }
    this.owners.delete(sessionId);
    return true;
  }

  releaseAll(owner: FrontendOwnerKind): void {
    for (const [sessionId, record] of this.owners) {
      if (record.owner === owner) {
        this.owners.delete(sessionId);
      }
    }
  }

  snapshot(): Map<string, FrontendOwnerKind> {
    return new Map(
      [...this.owners.entries()].map(([sessionId, record]) => [
        sessionId,
        record.owner,
      ]),
    );
  }
}
