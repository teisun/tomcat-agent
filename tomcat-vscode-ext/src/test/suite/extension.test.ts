import * as assert from "node:assert/strict";

import * as vscode from "vscode";

suite("Tomcat extension", () => {
  test("registers and activates the extension", async () => {
    const extension = vscode.extensions.getExtension("tomcat.tomcat-vscode-ext");

    assert.ok(extension, "expected extension to be discoverable by VS Code");
    await extension?.activate();
    assert.ok(extension?.isActive, "expected extension to activate");
  });
});
