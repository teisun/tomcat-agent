import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as vscode from "vscode";
import { openExtensionGuide } from "../src/extension";

vi.mock("node:fs/promises", async importOriginal => {
  const actual = await importOriginal<typeof import("node:fs/promises")>();
  return { ...actual, access: vi.fn(actual.access) };
});

const native = (vscode as typeof vscode & { __testing: typeof import("./stubs/vscode").__testing }).__testing;
const originalOpen = vscode.workspace.openTextDocument.bind(vscode.workspace);
let root: string;
let open: ReturnType<typeof vi.spyOn>;
let show: ReturnType<typeof vi.spyOn>;
const context = () => ({ extensionPath: root }) as vscode.ExtensionContext;

beforeEach(async () => {
  root = await fs.mkdtemp(path.join(os.tmpdir(), "tomcat-guide-test-"));
  native.reset();
  const actual = await vi.importActual<typeof import("node:fs/promises")>("node:fs/promises");
  vi.mocked(fs.access).mockReset().mockImplementation(actual.access);
  // The lightweight VS Code stub would otherwise open absent files as empty
  // documents. Read the real fixture first so missing files cannot falsely pass.
  open = vi.spyOn(vscode.workspace, "openTextDocument").mockImplementation(async input => {
    const uri = input as unknown as vscode.Uri;
    native.registerFile(uri.fsPath, await fs.readFile(uri.fsPath, "utf8"));
    return originalOpen(uri);
  });
  show = vi.spyOn(vscode.window, "showTextDocument");
});

afterEach(async () => {
  vi.restoreAllMocks();
  native.reset();
  await fs.rm(root, { recursive: true, force: true });
});

describe("installed extension guide", () => {
  it("opens the exact lowercase filename declared by the published VSIX", async () => {
    await fs.writeFile(path.join(root, "readme.md"), "packaged guide");
    await openExtensionGuide(context());
    const uri = open.mock.calls[0][0] as vscode.Uri;
    expect(uri.scheme).toBe("file");
    expect(uri.fsPath).toBe(path.join(root, "readme.md"));
    expect(vscode.window.activeTextEditor?.document.getText()).toBe("packaged guide");
    expect(show).toHaveBeenCalledWith(expect.anything(), { preview: true });
  });

  it("preserves source-checkout guides when the lowercase file is absent", async () => {
    await fs.writeFile(path.join(root, "README.md"), "source guide");
    // Simulate Linux's case-sensitive absence even on a case-insensitive Mac.
    vi.mocked(fs.access).mockRejectedValueOnce(Object.assign(new Error("missing packaged guide"), { code: "ENOENT" }));
    await openExtensionGuide(context());
    expect(fs.access).toHaveBeenCalledWith(path.join(root, "readme.md"));
    expect((open.mock.calls[0][0] as vscode.Uri).fsPath).toBe(path.join(root, "README.md"));
    expect(vscode.window.activeTextEditor?.document.getText()).toBe("source guide");
  });

  it("reports inaccessible packaged guides instead of silently falling back", async () => {
    await fs.writeFile(path.join(root, "README.md"), "must not mask access errors");
    const error = Object.assign(new Error("guide access denied"), { code: "EACCES" });
    vi.mocked(fs.access).mockRejectedValueOnce(error);
    await expect(openExtensionGuide(context())).rejects.toBe(error);
    expect(open).not.toHaveBeenCalled();
    expect(show).not.toHaveBeenCalled();
  });

  it("fails honestly when neither guide exists", async () => {
    await expect(openExtensionGuide(context())).rejects.toMatchObject({ code: "ENOENT" });
    expect(show).not.toHaveBeenCalled();
  });
});
