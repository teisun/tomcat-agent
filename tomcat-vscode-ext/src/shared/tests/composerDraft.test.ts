import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as vscode from "vscode";

import {
  ComposerDraftStore,
  EMPTY_DRAFT,
  isDraftEmpty,
  type DraftAttachmentRef,
} from "../composerDraft";

const { __testing } = vscode as unknown as {
  __testing: {
    readFile(path: string): string | undefined;
    registerFile(path: string, text: string): void;
    reset(): void;
  };
};

const ROOT = "/storage/composer-drafts";
const SESSION = "sid_abc123";

function draftPath(sessionId = SESSION): string {
  return `${ROOT}/${sessionId}.json`;
}

function attachment(overrides: Partial<DraftAttachmentRef> = {}): DraftAttachmentRef {
  return {
    blobSha: "a".repeat(64),
    bytes: 4_500_000,
    filename: "shot.png",
    hasThumb: true,
    id: "att-1",
    kind: "image",
    mimeType: "image/png",
    ...overrides,
  };
}

function newStore(debounceMs = 0): ComposerDraftStore {
  return new ComposerDraftStore(vscode.Uri.file(ROOT), debounceMs);
}

beforeEach(() => {
  __testing.reset();
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("ComposerDraftStore storage location", () => {
  it("prefers workspace storage and falls back to global for an empty window", () => {
    const withWorkspace = ComposerDraftStore.forContext({
      globalStorageUri: vscode.Uri.file("/global"),
      storageUri: vscode.Uri.file("/workspace-storage"),
    });
    const withoutWorkspace = ComposerDraftStore.forContext({
      globalStorageUri: vscode.Uri.file("/global"),
      storageUri: undefined,
    });

    // Reaching into the write path is the only observable difference, so drive one.
    withWorkspace.update(SESSION, (draft) => ({ ...draft, text: "in workspace" }));
    withoutWorkspace.update(SESSION, (draft) => ({ ...draft, text: "in global" }));
    vi.runAllTimers();

    return Promise.all([withWorkspace.flush(), withoutWorkspace.flush()]).then(() => {
      expect(
        __testing.readFile(`/workspace-storage/composer-drafts/${SESSION}.json`),
      ).toContain("in workspace");
      expect(__testing.readFile(`/global/composer-drafts/${SESSION}.json`)).toContain(
        "in global",
      );
    });
  });

  it("refuses to build a path from a session id that could escape the directory", () => {
    const store = newStore();
    expect(() => store.update("../../etc/passwd", (draft) => draft)).toThrow(
      /refusing to build a draft path/,
    );
  });
});

describe("ComposerDraftStore persistence", () => {
  it("writes once for a burst of keystrokes", async () => {
    const store = newStore(400);
    const writeFile = vi.spyOn(vscode.workspace.fs, "writeFile");

    for (const text of ["h", "he", "hel", "hell", "hello"]) {
      store.update(SESSION, (draft) => ({ ...draft, text }));
    }
    // Nothing has touched the disk yet: this is the whole point of the debounce.
    expect(writeFile).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(400);
    await store.flush();

    expect(writeFile).toHaveBeenCalledTimes(1);
    expect(__testing.readFile(draftPath())).toContain('"text":"hello"');
  });

  it("stores attachment references and no bytes at all", async () => {
    const store = newStore(0);
    store.update(SESSION, (draft) => ({
      ...draft,
      attachments: [attachment({ sourcePath: "/workspace/assets/shot.png" })],
      text: "look at this",
    }));
    await vi.runAllTimersAsync();
    await store.flush();

    const persisted = __testing.readFile(draftPath()) ?? "";
    expect(JSON.parse(persisted)).toMatchObject({
      attachments: [{ blobSha: "a".repeat(64), hasThumb: true, sourcePath: "/workspace/assets/shot.png" }],
      text: "look at this",
    });
    // The contract that keeps a draft ~200 bytes instead of ~4MB per image.
    expect(persisted).not.toContain("dataBase64");
    expect(persisted).not.toContain("base64");
    expect(persisted.length).toBeLessThan(1024);
  });

  it("writes through a temp file so an interrupted write cannot corrupt the draft", async () => {
    const store = newStore(0);
    store.update(SESSION, (draft) => ({ ...draft, text: "first" }));
    await vi.runAllTimersAsync();
    await store.flush();

    // Fail the rename, i.e. crash after the temp file exists but before it lands.
    const rename = vi
      .spyOn(vscode.workspace.fs, "rename")
      .mockRejectedValueOnce(new Error("disk full"));
    store.update(SESSION, (draft) => ({ ...draft, text: "second" }));
    await vi.runAllTimersAsync();
    await store.flush();

    expect(rename).toHaveBeenCalled();
    // The previous draft survived intact, and no debris was left behind.
    expect(__testing.readFile(draftPath())).toContain('"text":"first"');
    expect(__testing.readFile(`${draftPath()}.tmp`)).toBeUndefined();
  });

  it("deletes the file when the draft becomes empty rather than leaving a husk", async () => {
    const store = newStore(0);
    store.update(SESSION, (draft) => ({ ...draft, text: "typed" }));
    await vi.runAllTimersAsync();
    await store.flush();
    expect(__testing.readFile(draftPath())).toBeDefined();

    store.update(SESSION, (draft) => ({ ...draft, text: "" }));
    await vi.runAllTimersAsync();
    await store.flush();

    // An empty file would hydrate back as "there is a draft", making a cleared composer
    // look like an unsaved one.
    expect(__testing.readFile(draftPath())).toBeUndefined();
  });
});

describe("ComposerDraftStore hydration", () => {
  it("restores text, segments and attachment references from disk", async () => {
    __testing.registerFile(
      draftPath(),
      JSON.stringify({
        attachments: [attachment()],
        schemaVersion: 2,
        segments: [
          { kind: "file", label: "main.rs", path: "/repo/main.rs", type: "reference" },
        ],
        text: "about @main.rs",
        updatedAt: Date.now(),
      }),
    );

    const draft = await newStore().hydrate(SESSION);

    expect(draft.text).toBe("about @main.rs");
    expect(draft.segments).toHaveLength(1);
    expect(draft.attachments[0]?.blobSha).toBe("a".repeat(64));
    expect(draft.attachments[0]?.sourcePath).toBeNull();
  });

  it("keeps a newer edit when an older disk read finishes later", async () => {
    const store = newStore(400);
    let finishRead: ((value: Uint8Array) => void) | undefined;
    const delayedRead = new Promise<Uint8Array>((resolve) => {
      finishRead = resolve;
    });
    vi.spyOn(vscode.workspace.fs, "readFile").mockImplementationOnce(
      async () => delayedRead,
    );

    const loading = store.hydrate(SESSION);
    store.update(SESSION, (draft) => ({ ...draft, text: "newer text" }));
    finishRead?.(new TextEncoder().encode(JSON.stringify({
      attachments: [],
      schemaVersion: 2,
      segments: [],
      text: "older text",
    })));

    expect((await loading).text).toBe("newer text");
    expect(store.peek(SESSION).text).toBe("newer text");
  });

  it("shares concurrent cold reads for the same session", async () => {
    __testing.registerFile(
      draftPath(),
      JSON.stringify({ schemaVersion: 2, segments: [], text: "one disk read" }),
    );
    const readFile = vi.spyOn(vscode.workspace.fs, "readFile");
    const store = newStore();

    const [first, second] = await Promise.all([store.hydrate(SESSION), store.hydrate(SESSION)]);

    expect(first.text).toBe("one disk read");
    expect(second.text).toBe("one disk read");
    expect(readFile).toHaveBeenCalledTimes(1);
  });

  it("keeps an explicit clear when an older cold read completes afterward", async () => {
    const store = newStore();
    let finishRead: ((value: Uint8Array) => void) | undefined;
    const delayedRead = new Promise<Uint8Array>((resolve) => {
      finishRead = resolve;
    });
    vi.spyOn(vscode.workspace.fs, "readFile").mockImplementationOnce(async () => delayedRead);

    const loading = store.hydrate(SESSION);
    await store.discard(SESSION);
    finishRead?.(new TextEncoder().encode(JSON.stringify({
      schemaVersion: 2,
      segments: [],
      text: "discarded old text",
    })));

    expect(await loading).toEqual(EMPTY_DRAFT);
    expect(store.peek(SESSION)).toEqual(EMPTY_DRAFT);
  });

  it("writes a newer edit after a corrupt draft is isolated", async () => {
    __testing.registerFile(draftPath(), "{ broken json");
    const store = newStore(0);
    const originalRename = vscode.workspace.fs.rename.bind(vscode.workspace.fs);
    let releaseRename: (() => void) | undefined;
    const pausedRename = new Promise<void>((resolve) => {
      releaseRename = resolve;
    });
    const rename = vi.spyOn(vscode.workspace.fs, "rename").mockImplementationOnce(async (from, to, options) => {
      await pausedRename;
      await originalRename(from, to, options);
    });

    const loading = store.hydrate(SESSION);
    for (let index = 0; index < 4; index += 1) {
      await Promise.resolve();
    }
    expect(rename).toHaveBeenCalledTimes(1);
    store.update(SESSION, (draft) => ({ ...draft, text: "new after corrupt read" }));
    releaseRename?.();
    await loading;
    await vi.runAllTimersAsync();
    await store.flush();

    expect(__testing.readFile(draftPath())).toContain("new after corrupt read");
  });

  it("quarantines an unreadable draft and still hands back a usable composer", async () => {
    __testing.registerFile(draftPath(), "{ this is not json");

    const draft = await newStore().hydrate(SESSION);

    expect(draft).toEqual(EMPTY_DRAFT);
    // Moved aside, not deleted: the user can type immediately and the bad file is still
    // there to be looked at.
    expect(__testing.readFile(draftPath())).toBeUndefined();
    expect(__testing.readFile(`${draftPath()}.corrupt`)).toBe("{ this is not json");
  });

  it("refuses a draft written by a newer schema instead of silently dropping fields", async () => {
    __testing.registerFile(
      draftPath(),
      JSON.stringify({ schemaVersion: 99, text: "from the future" }),
    );

    expect(await newStore().hydrate(SESSION)).toEqual(EMPTY_DRAFT);
  });

  it("keeps the surrounding draft when one attachment entry is unreadable", async () => {
    __testing.registerFile(
      draftPath(),
      JSON.stringify({
        attachments: [attachment(), { id: "att-2", kind: "image" }],
        schemaVersion: 2,
        segments: [],
        text: "five minutes of typing",
      }),
    );

    const draft = await newStore().hydrate(SESSION);

    // Losing one image must not cost the paragraph written around it.
    expect(draft.text).toBe("five minutes of typing");
    expect(draft.attachments).toHaveLength(1);
  });

  it("restores sourcePath when present and nulls out dirty non-string values", async () => {
    __testing.registerFile(
      draftPath(),
      JSON.stringify({
        attachments: [
          attachment({ sourcePath: "/workspace/assets/shot.png" }),
          { ...attachment({ id: "att-2" }), sourcePath: { nope: true } },
        ],
        schemaVersion: 2,
        segments: [],
        text: "files",
        updatedAt: Date.now(),
      }),
    );

    const draft = await newStore().hydrate(SESSION);

    expect(draft.attachments[0]?.sourcePath).toBe("/workspace/assets/shot.png");
    expect(draft.attachments[1]?.sourcePath).toBeNull();
  });

  it("drops a dangling draft whose session no longer exists", async () => {
    __testing.registerFile(
      draftPath("sid_deleted"),
      JSON.stringify({ schemaVersion: 2, segments: [], text: "orphan" }),
    );

    const draft = await newStore().hydrate("sid_deleted", () => false);

    expect(draft).toEqual(EMPTY_DRAFT);
    expect(__testing.readFile(draftPath("sid_deleted"))).toBeUndefined();
  });
});

describe("ComposerDraftStore lifecycle", () => {
  it("survives a host shutdown before the send is acknowledged", async () => {
    const store = newStore(400);
    store.update(SESSION, (draft) => ({
      ...draft,
      attachments: [attachment()],
      text: "unsent",
    }));

    // Deactivation flushes without waiting for the debounce to expire.
    await store.flush();
    expect(__testing.readFile(draftPath())).toContain('"text":"unsent"');

    // A fresh store, as after a window reload, sees the same draft.
    expect((await newStore().hydrate(SESSION)).text).toBe("unsent");
  });

  it("discards only on acknowledgement, and only the acknowledged session", async () => {
    const store = newStore(0);
    store.update(SESSION, (draft) => ({ ...draft, text: "sent" }));
    store.update("sid_other", (draft) => ({ ...draft, text: "still typing" }));
    await vi.runAllTimersAsync();
    await store.flush();

    await store.discard(SESSION);

    expect(__testing.readFile(draftPath())).toBeUndefined();
    expect(__testing.readFile(draftPath("sid_other"))).toContain("still typing");
    expect(store.peek(SESSION)).toEqual(EMPTY_DRAFT);
  });

  it("does not clear a later edit that returns to the submitted text", async () => {
    const store = newStore(0);
    store.update(SESSION, (draft) => ({ ...draft, text: "same words" }));
    const submittedRevision = store.currentRevision(SESSION);

    store.update(SESSION, (draft) => ({ ...draft, text: "temporary edit" }));
    store.update(SESSION, (draft) => ({ ...draft, text: "same words" }));

    expect(await store.discardIfRevision(SESSION, submittedRevision)).toBe(false);
    expect(store.peek(SESSION).text).toBe("same words");
  });

  it("cancels a scheduled write when the draft is discarded first", async () => {
    const store = newStore(400);
    const writeFile = vi.spyOn(vscode.workspace.fs, "writeFile");
    store.update(SESSION, (draft) => ({ ...draft, text: "about to be sent" }));

    await store.discard(SESSION);
    await vi.advanceTimersByTimeAsync(400);

    // A write landing after the discard would resurrect a draft the user already sent.
    expect(writeFile).not.toHaveBeenCalled();
    expect(__testing.readFile(draftPath())).toBeUndefined();
  });
});

describe("ComposerDraftStore fork install", () => {
  it("installs a deep copy into a cold empty target and rejects same-process replay", async () => {
    const store = newStore(0);
    const source = {
      attachments: [attachment()],
      segments: [{ text: "draft", type: "text" as const }],
      text: "draft",
    };

    const installed = await store.installIfEmpty("sid_target", source);
    source.attachments[0]!.filename = "mutated.png";
    source.segments[0] = { text: "mutated", type: "text" };

    expect(installed.attachments[0]?.filename).toBe("shot.png");
    expect(store.peek("sid_target").segments[0]).toEqual({ text: "draft", type: "text" });
    expect(__testing.readFile(draftPath("sid_target"))).toContain('"text":"draft"');
    await expect(store.installIfEmpty("sid_target", installed)).rejects.toThrow("draft conflict");
  });

  it("checks cold disk state and rejects non-empty or corrupt targets", async () => {
    __testing.registerFile(
      draftPath("sid_existing"),
      JSON.stringify({ attachments: [], schemaVersion: 2, segments: [], text: "existing" }),
    );
    __testing.registerFile(draftPath("sid_corrupt"), "not json");
    const store = newStore(0);
    const candidate = { attachments: [], segments: [], text: "candidate" };

    await expect(store.installIfEmpty("sid_existing", candidate)).rejects.toThrow("not empty on disk");
    await expect(store.installIfEmpty("sid_corrupt", candidate)).rejects.toThrow("invalid persisted data");
    expect(__testing.readFile(draftPath("sid_existing"))).toContain("existing");
    expect(__testing.readFile(draftPath("sid_corrupt"))).toBe("not json");
  });

  it("propagates transaction writes and restores memory after I/O failure", async () => {
    const store = newStore(0);
    const rename = vi.spyOn(vscode.workspace.fs, "rename").mockRejectedValueOnce(new Error("disk full"));

    await expect(
      store.installIfEmpty("sid_io_failure", { attachments: [], segments: [], text: "candidate" }),
    ).rejects.toThrow("disk full");
    expect(store.peek("sid_io_failure")).toEqual(EMPTY_DRAFT);
    expect(__testing.readFile(draftPath("sid_io_failure"))).toBeUndefined();
    expect(rename).toHaveBeenCalled();
  });

  it("strictly removes a partially installed target and surfaces cleanup I/O failures", async () => {
    const store = newStore(0);
    await store.installIfEmpty("sid_partial", {
      attachments: [attachment()],
      segments: [],
      text: "target",
    });
    await store.discardStrict("sid_partial");
    expect(store.peek("sid_partial")).toEqual(EMPTY_DRAFT);
    expect(__testing.readFile(draftPath("sid_partial"))).toBeUndefined();
    await expect(store.discardStrict("sid_partial")).resolves.toBeUndefined();

    await store.installIfEmpty("sid_cleanup_error", {
      attachments: [],
      segments: [],
      text: "target",
    });
    vi.spyOn(vscode.workspace.fs, "delete").mockRejectedValueOnce(new Error("permission denied"));
    await expect(store.discardStrict("sid_cleanup_error")).rejects.toThrow("permission denied");
    expect(store.peek("sid_cleanup_error").text).toBe("target");
  });

  it("reloads a committed target draft from a fresh store", async () => {
    const first = newStore(0);
    await first.installIfEmpty("sid_reload", {
      attachments: [attachment({ providerSha: "b".repeat(64) })],
      segments: [{ kind: "file", label: "main.rs", path: "/repo/main.rs", type: "reference" }],
      text: "reload me",
    });

    const reloaded = await newStore(0).hydrate("sid_reload");
    expect(reloaded.text).toBe("reload me");
    expect(reloaded.attachments[0]).toMatchObject({
      blobSha: "a".repeat(64),
      providerSha: "b".repeat(64),
    });
    expect(reloaded.segments[0]).toMatchObject({ path: "/repo/main.rs", type: "reference" });
  });

  it("durably replaces the source snapshot and propagates failures", async () => {
    const store = newStore(0);
    await store.replaceAndFlush(SESSION, { attachments: [], segments: [], text: "source" });
    expect(__testing.readFile(draftPath())).toContain('"text":"source"');

    vi.spyOn(vscode.workspace.fs, "rename").mockRejectedValueOnce(new Error("readonly"));
    await expect(
      store.replaceAndFlush(SESSION, { attachments: [], segments: [], text: "new" }),
    ).rejects.toThrow("readonly");
    expect(store.peek(SESSION).text).toBe("source");
    expect(__testing.readFile(draftPath())).toContain('"text":"source"');
  });
});

describe("isDraftEmpty", () => {
  it("treats an attachment-only draft as non-empty", () => {
    expect(isDraftEmpty(EMPTY_DRAFT)).toBe(true);
    expect(
      isDraftEmpty({ attachments: [attachment()], segments: [], text: "" }),
    ).toBe(false);
  });
});
