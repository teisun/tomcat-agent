import * as fs from "node:fs/promises";
import * as path from "node:path";

import { describe, expect, it } from "vitest";

import {
  PARTICIPANT_ID,
} from "../src/constants";

type Manifest = {
  activationEvents?: string[];
  contributes?: {
    configuration?: {
      properties?: Record<string, unknown>;
    };
    chatParticipants?: Array<Record<string, unknown>>;
    views?: Record<string, Array<Record<string, unknown>>>;
    viewsContainers?: Record<string, Array<Record<string, unknown>>>;
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

  it("declares the participant, command, and webview activation events", async () => {
    const manifest = await readManifest();

    expect(manifest.activationEvents).toEqual(
      expect.arrayContaining([
        `onChatParticipant:${PARTICIPANT_ID}`,
        "onCommand:tomcat.ui.focus",
        "onView:tomcat.chatView",
      ]),
    );
  });

  it("keeps package:vsix wired to the shared packaging script", async () => {
    const manifest = await readManifest();

    expect(manifest.scripts?.["package:vsix"]).toBe("tsx scripts/package-vsix.ts");
  });

  it("registers the stable slash commands for the chat participant", async () => {
    const manifest = await readManifest();
    const participant = manifest.contributes?.chatParticipants?.[0];
    const commands = Array.isArray(participant?.commands)
      ? participant.commands
      : [];

    expect(commands).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ name: "plan" }),
        expect.objectContaining({ name: "model" }),
      ]),
    );
  });

  it("declares the Tomcat webview container and view", async () => {
    const manifest = await readManifest();
    const containers = manifest.contributes?.viewsContainers?.activitybar ?? [];
    const views = manifest.contributes?.views?.["tomcat-sidebar"] ?? [];

    expect(containers).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ id: "tomcat-sidebar", title: "Tomcat" }),
      ]),
    );
    expect(views).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          id: "tomcat.chatView",
          name: "Tomcat",
          type: "webview",
        }),
      ]),
    );
  });

  it("declares the ui mode configuration contract", async () => {
    const manifest = await readManifest();
    const uiSetting = manifest.contributes?.configuration?.properties?.["tomcat.ui"] as
      | { default?: string; enum?: string[] }
      | undefined;

    expect(uiSetting?.default).toBe("both");
    expect(uiSetting?.enum).toEqual(["both", "participant", "webview"]);
  });
});
