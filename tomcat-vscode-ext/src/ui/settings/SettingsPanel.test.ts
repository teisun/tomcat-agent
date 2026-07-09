import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";

import { afterEach, describe, expect, it } from "vitest";
import * as vscode from "vscode";

import { SettingsPanel } from "./SettingsPanel";

describe("settings panel html asset resolution", () => {
  const tempDirs: string[] = [];

  afterEach(async () => {
    await Promise.all(
      tempDirs.map(async (dir) => {
        await fs.rm(dir, { force: true, recursive: true });
      }),
    );
    tempDirs.length = 0;
  });

  async function createExtensionRoot(files: Record<string, string>): Promise<vscode.Uri> {
    const dir = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-settings-assets-"));
    tempDirs.push(dir);
    await Promise.all(
      Object.entries(files).map(async ([relativePath, contents]) => {
        const filePath = path.join(dir, relativePath);
        await fs.mkdir(path.dirname(filePath), { recursive: true });
        await fs.writeFile(filePath, contents, "utf8");
      }),
    );
    return vscode.Uri.file(dir);
  }

  function createWebview(): vscode.Webview {
    return {
      asWebviewUri(uri: vscode.Uri) {
        return uri;
      },
      cspSource: "vscode-test-webview",
    } as unknown as vscode.Webview;
  }

  it("falls back to another built stylesheet when styles.css is absent", async () => {
    const extensionUri = await createExtensionRoot({
      "gui/dist/settings.js": "console.log('settings');",
      "gui/dist/theme.css": "body { color: blue; }",
    });
    const panel = new SettingsPanel({
      ensureInitialized: async () => ({} as never),
      extensionUri,
      messenger: {} as never,
    });

    const html = (
      panel as unknown as {
        renderHtml(webview: vscode.Webview): string;
      }
    ).renderHtml(createWebview());

    expect(html).toContain('rel="stylesheet"');
    expect(html).toContain("theme.css");
  });
});
