import * as fs from "node:fs/promises";
import * as path from "node:path";

import { describe, expect, it } from "vitest";

type Manifest = {
  activationEvents?: string[];
  contributes?: {
    configuration?: {
      properties?: Record<string, unknown>;
    };
    commands?: Array<Record<string, unknown>>;
    keybindings?: Array<Record<string, unknown>>;
    menus?: Record<string, Array<Record<string, unknown>>>;
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
  it("does not contribute a chat participant after the webview-only migration", async () => {
    const manifest = await readManifest();

    expect(manifest.contributes).not.toHaveProperty("chatParticipants");
    expect(manifest.contributes).not.toHaveProperty("languageModelChatProviders");
    for (const event of manifest.activationEvents ?? []) {
      expect(event.startsWith("onChatParticipant:")).toBe(false);
    }
  });

  it("activates on startup", async () => {
    const manifest = await readManifest();

    expect(manifest.activationEvents).toEqual(
      expect.arrayContaining(["onStartupFinished"]),
    );
  });

  it("keeps package:vsix wired to the shared packaging script", async () => {
    const manifest = await readManifest();

    expect(manifest.scripts?.["package:vsix"]).toBe("tsx scripts/package-vsix.ts");
  });

  it("keeps fast/full extension gate scripts wired to the shared entrypoints", async () => {
    const manifest = await readManifest();

    expect(manifest.scripts?.["test:unit:core"]).toBe("vitest run --maxWorkers 4 src");
    expect(manifest.scripts?.["test:integration"]).toBe("vitest run --maxWorkers 1 tests");
    expect(manifest.scripts?.["gate:fast"]).toBe("npm run lint && npm run test:unit");
    expect(manifest.scripts?.["gate:full"]).toBe("tsx scripts/run-vscode-full-gate.ts");
  });

  it("declares the Tomcat webview container and view", async () => {
    const manifest = await readManifest();
    const containers = manifest.contributes?.viewsContainers?.secondarySidebar ?? [];
    const views = manifest.contributes?.views?.["tomcat-sidebar"] ?? [];

    expect(containers).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ id: "tomcat-sidebar", title: "TOMCAT" }),
      ]),
    );
    expect(views).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          id: "tomcat.chatView",
          name: "TOMCAT",
          type: "webview",
        }),
      ]),
    );
  });

  it("does not declare the removed tomcat.ui configuration", async () => {
    const manifest = await readManifest();

    expect(manifest.contributes?.configuration?.properties).not.toHaveProperty(
      "tomcat.ui",
    );
  });

  it("registers add-to-chat commands and affordances", async () => {
    const manifest = await readManifest();
    const commands = manifest.contributes?.commands ?? [];
    const editorMenus = manifest.contributes?.menus?.["editor/context"] ?? [];
    const explorerMenus = manifest.contributes?.menus?.["explorer/context"] ?? [];
    const keybindings = manifest.contributes?.keybindings ?? [];

    expect(commands).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ command: "tomcat.openSettings" }),
        expect.objectContaining({ command: "tomcat.addSelectionToChat" }),
        expect.objectContaining({ command: "tomcat.addFileToChat" }),
      ]),
    );
    expect(editorMenus).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ command: "tomcat.addSelectionToChat" }),
      ]),
    );
    expect(explorerMenus).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ command: "tomcat.addFileToChat" }),
      ]),
    );
    const explorerAddFileMenu = explorerMenus.find(
      (entry: { command?: string; when?: string }) => entry.command === "tomcat.addFileToChat",
    );
    expect(explorerAddFileMenu?.when).toBeUndefined();
    expect(keybindings).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          command: "tomcat.addSelectionToChat",
          key: "ctrl+alt+a",
          mac: "cmd+alt+a",
        }),
      ]),
    );
  });
});
