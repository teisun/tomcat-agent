import * as fs from "node:fs/promises";
import * as path from "node:path";

import { describe, expect, it } from "vitest";

import {
  PARTICIPANT_ID,
} from "../src/constants";

type Manifest = {
  activationEvents?: string[];
  contributes?: {
    chatParticipants?: Array<Record<string, unknown>>;
  };
  scripts?: Record<string, string>;
};

async function readManifest(): Promise<Manifest> {
  const manifestPath = path.resolve(__dirname, "..", "package.json");
  return JSON.parse(await fs.readFile(manifestPath, "utf8")) as Manifest;
}

describe("extension manifest contract", () => {
  it("keeps the chat participant id aligned with the runtime constant", async () => {
    const manifest = await readManifest();
    const participant = manifest.contributes?.chatParticipants?.[0];

    expect(participant?.id).toBe(PARTICIPANT_ID);
  });

  it("does not reintroduce proposed-only chat participant fields", async () => {
    const manifest = await readManifest();
    const participant = manifest.contributes?.chatParticipants?.[0] ?? {};

    expect(participant).not.toHaveProperty("isDefault");
    expect(participant).not.toHaveProperty("modes");
  });

  it("keeps only the explicit chat participant activation event", async () => {
    const manifest = await readManifest();

    expect(manifest.activationEvents).toEqual([`onChatParticipant:${PARTICIPANT_ID}`]);
  });

  it("keeps package:vsix wired to the shared packaging script", async () => {
    const manifest = await readManifest();

    expect(manifest.scripts?.["package:vsix"]).toBe("tsx scripts/package-vsix.ts");
  });
});
