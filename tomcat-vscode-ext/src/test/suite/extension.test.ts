import * as assert from "node:assert/strict";

import * as vscode from "vscode";
import { EXTENSION_ID } from "../../constants";

suite("Tomcat extension", () => {
  test("registers and activates the extension", async () => {
    const extension = vscode.extensions.getExtension(EXTENSION_ID);

    assert.ok(extension, "expected extension to be discoverable by VS Code");
    await extension?.activate();
    assert.ok(extension?.isActive, "expected extension to activate");
  });
});
