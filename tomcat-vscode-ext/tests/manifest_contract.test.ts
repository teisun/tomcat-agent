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
    customEditors?: Array<{
      priority?: string;
      selector?: Array<{ filenamePattern?: string }>;
      viewType?: string;
    }>;
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
  it("resolves every manifest contribution in both static dictionaries", async () => {
    const root = path.resolve(__dirname, "..");
    const manifest = await fs.readFile(path.join(root, "package.json"), "utf8");
    const english = JSON.parse(await fs.readFile(path.join(root, "package.nls.json"), "utf8")) as Record<string, string>;
    const chinese = JSON.parse(await fs.readFile(path.join(root, "package.nls.zh-cn.json"), "utf8")) as Record<string, string>;
    const keys = [...manifest.matchAll(/"%([a-zA-Z][\w.]+)%"/gu)].map((match) => match[1]);
    expect(keys.length).toBeGreaterThan(10);
    expect(Object.keys(english).sort()).toEqual(Object.keys(chinese).sort());
    expect([...new Set(keys)].sort()).toEqual(Object.keys(english).sort());
    for (const key of keys) {
      expect(english[key].trim()).not.toBe("");
      expect(chinese[key].trim()).not.toBe("");
    }
    const ignore = await fs.readFile(path.join(root, ".vscodeignore"), "utf8");
    expect(ignore).toContain("!package.nls.json");
    expect(ignore).toContain("!package.nls.zh-cn.json");
  });

  it("declares 15/25 layout defaults with bounded integer settings", async () => {
    const properties = (await readManifest()).contributes?.configuration?.properties;
    for (const [setting, defaultValue] of [
      ["tomcat.layout.controlsInset", 15],
      ["tomcat.layout.contentInset", 25],
    ] as const) {
      expect(properties?.[setting]).toMatchObject({ type: "integer", minimum: 0, maximum: 40, default: defaultValue });
    }
  });

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
    expect(manifest.scripts?.["test:integration"]).toBe("vitest run --config vitest.integration.config.ts --maxWorkers 1");
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

  it("contributes .plan.md files to the default Plan preview editor", async () => {
    const manifest = await readManifest();
    const editor = manifest.contributes?.customEditors?.find(
      (candidate) => candidate.viewType === "tomcat.planPreview",
    );

    expect(editor).toEqual(
      expect.objectContaining({
        priority: "default",
        selector: expect.arrayContaining([
          expect.objectContaining({ filenamePattern: "*.plan.md" }),
        ]),
        viewType: "tomcat.planPreview",
      }),
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
