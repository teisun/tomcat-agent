import * as fsPromises from "node:fs/promises";
import * as os from "node:os";
import path from "node:path";

import { afterEach, describe, expect, it, vi } from "vitest";
import * as vscode from "vscode";

import type { InitializeResult } from "../../../serveClient/initialize";
import type {
  PlanActivePanelInfo,
  PlanPreviewDocumentLike,
  PlanPreviewEditorProviderDeps,
} from "../PlanPreviewEditorProvider";
import {
  PlanPreviewEditorProvider,
  classifyPlanLink,
  deriveCanBuild,
} from "../PlanPreviewEditorProvider";
import type {
  PlanPreviewHostFrame,
  PlanPreviewStateSnapshot,
} from "../../../shared/planPreviewProtocol";

const __testing = (
  vscode as typeof vscode & {
    __testing: {
      reset(): void;
      setConfiguration(key: string, value: unknown): void;
    };
  }
).__testing;

class FakeWebview {
  cspSource = "vscode-webview:";
  html = "";
  options: unknown;
  readonly messages: unknown[] = [];
  private readonly receiveEmitter = new vscode.EventEmitter<unknown>();
  readonly onDidReceiveMessage = this.receiveEmitter.event;

  asWebviewUri(uri: vscode.Uri): vscode.Uri {
    return uri;
  }

  async postMessage(message: unknown): Promise<boolean> {
    this.messages.push(message);
    return true;
  }

  stateFrames(): PlanPreviewStateSnapshot[] {
    return (this.messages as PlanPreviewHostFrame[])
      .filter((frame) => frame.channel === "state")
      .map((frame) => frame.content as PlanPreviewStateSnapshot);
  }

  lastState(): PlanPreviewStateSnapshot | undefined {
    const frames = this.stateFrames();
    return frames[frames.length - 1];
  }
}

class FakeWebviewPanel {
  active: boolean;
  readonly webview = new FakeWebview();
  private readonly viewStateEmitter = new vscode.EventEmitter<void>();
  private readonly disposeEmitter = new vscode.EventEmitter<void>();
  readonly onDidChangeViewState = this.viewStateEmitter.event;
  readonly onDidDispose = this.disposeEmitter.event;

  constructor(active = true) {
    this.active = active;
  }

  setActive(active: boolean): void {
    this.active = active;
    this.viewStateEmitter.fire();
  }

  fireDispose(): void {
    this.disposeEmitter.fire();
  }
}

function fakeDocument(text: string, docPath: string) {
  return { getText: () => text, uri: vscode.Uri.file(docPath) };
}

async function flush(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0));
  await new Promise((resolve) => setTimeout(resolve, 0));
}

async function resolveEditor(
  provider: PlanPreviewEditorProvider,
  text: string,
  docPath: string,
  active = true,
): Promise<{ panel: FakeWebviewPanel }> {
  const panel = new FakeWebviewPanel(active);
  provider.resolveCustomTextEditor(
    fakeDocument(text, docPath) as unknown as vscode.TextDocument,
    panel as unknown as vscode.WebviewPanel,
  );
  await flush();
  return { panel };
}

const PLAN_TEXT = `---
plan_id: plan-xyz
name: Sample Plan
overview: One-line overview
state: planning
todos:
- id: t1
  content: First task
  status: pending
- id: t2
  content: Second task
  status: completed
---

# Heading

Body paragraph.
`;

function initResult(capabilities: string[]): InitializeResult {
  return { capabilities, protocolVersion: 1, sessionId: null };
}

function makeDeps(
  overrides: Partial<PlanPreviewEditorProviderDeps> = {},
): PlanPreviewEditorProviderDeps {
  return {
    addSelectionToChat: vi.fn().mockResolvedValue(undefined),
    buildPlan: vi.fn().mockResolvedValue(undefined),
    ensureInitialized: vi
      .fn()
      .mockResolvedValue(initResult(["set_plan_mode", "list_models"])),
    extensionUri: vscode.Uri.file("/workspace/extension"),
    getBuildModel: vi.fn().mockReturnValue(""),
    messenger: {
      sendListModels: vi.fn().mockResolvedValue({
        payload: { models: [{ id: "gpt-5.4", keyPresent: true }] },
        success: true,
      }),
    } as never,
    openExternal: vi.fn().mockResolvedValue(undefined),
    openInTextEditor: vi.fn().mockResolvedValue(undefined),
    openWorkspaceFile: vi.fn().mockResolvedValue(undefined),
    setBuildModel: vi.fn().mockResolvedValue(undefined),
    ...overrides,
  };
}

function makeDoc(text = PLAN_TEXT, docPath = "/workspace/plans/sample.plan.md"): PlanPreviewDocumentLike {
  return { getText: () => text, path: docPath };
}

describe("deriveCanBuild", () => {
  it("is false whenever the serve lacks set_plan_mode", () => {
    expect(deriveCanBuild("planning", false)).toBe(false);
    expect(deriveCanBuild("pending", false)).toBe(false);
  });

  it("is true only for planning/pending states when capable", () => {
    expect(deriveCanBuild("planning", true)).toBe(true);
    expect(deriveCanBuild("pending", true)).toBe(true);
    expect(deriveCanBuild("executing", true)).toBe(false);
    expect(deriveCanBuild("completed", true)).toBe(false);
    expect(deriveCanBuild(null, true)).toBe(false);
  });
});

describe("classifyPlanLink", () => {
  const planPath = "/workspace/plans/sample.plan.md";

  it("treats http(s) and mailto and other schemes as external", () => {
    expect(classifyPlanLink("https://example.com", planPath)).toEqual({
      href: "https://example.com",
      kind: "external",
    });
    expect(classifyPlanLink("mailto:a@b.com", planPath)).toEqual({
      href: "mailto:a@b.com",
      kind: "external",
    });
    expect(classifyPlanLink("vscode://tomcat/thing", planPath)).toEqual({
      href: "vscode://tomcat/thing",
      kind: "external",
    });
  });

  it("ignores empty links and pure anchors", () => {
    expect(classifyPlanLink("", planPath)).toEqual({ kind: "ignore" });
    expect(classifyPlanLink("   ", planPath)).toEqual({ kind: "ignore" });
    expect(classifyPlanLink("#section", planPath)).toEqual({ kind: "ignore" });
  });

  it("resolves relative and absolute file links against the plan directory", () => {
    expect(classifyPlanLink("docs/design.md", planPath)).toEqual({
      kind: "file",
      path: path.resolve("/workspace/plans", "docs/design.md"),
    });
    expect(classifyPlanLink("/abs/notes.md", planPath)).toEqual({
      kind: "file",
      path: "/abs/notes.md",
    });
    expect(classifyPlanLink("neighbor.md#heading", planPath)).toEqual({
      kind: "file",
      path: path.resolve("/workspace/plans", "neighbor.md"),
    });
  });
});

describe("PlanPreviewEditorProvider.buildState", () => {
  it("maps parsed document + capabilities into a snapshot", async () => {
    const provider = new PlanPreviewEditorProvider(makeDeps());
    const snapshot = await provider.buildState(PLAN_TEXT, "/workspace/plans/sample.plan.md");

    expect(snapshot.title).toBe("Sample Plan");
    expect(snapshot.overview).toBe("One-line overview");
    expect(snapshot.planId).toBe("plan-xyz");
    expect(snapshot.state).toBe("planning");
    expect(snapshot.path).toBe("/workspace/plans/sample.plan.md");
    expect(snapshot.todos).toEqual([
      { content: "First task", id: "t1", status: "pending" },
      { content: "Second task", id: "t2", status: "completed" },
    ]);
    expect(snapshot.availableModels).toEqual(["gpt-5.4"]);
    expect(snapshot.canBuild).toBe(true);
    expect(snapshot.bodyMarkdown).toContain("# Heading");
    // `# Heading` is line 15 and `Body paragraph.` line 17 of PLAN_TEXT.
    const bodyLines = snapshot.bodyMarkdown.split("\n");
    expect(snapshot.bodyLineMap).toHaveLength(bodyLines.length);
    expect(snapshot.bodyLineMap[0]).toBe(15);
    expect(snapshot.bodyLineMap[bodyLines.indexOf("Body paragraph.")]).toBe(17);
  });

  it("keeps the build model when it is a known model", async () => {
    const provider = new PlanPreviewEditorProvider(
      makeDeps({ getBuildModel: vi.fn().mockReturnValue("gpt-5.4") }),
    );
    const snapshot = await provider.buildState(PLAN_TEXT, "/p/sample.plan.md");
    expect(snapshot.buildModel).toBe("gpt-5.4");
  });

  it("drops a build model that is not in the available list", async () => {
    const provider = new PlanPreviewEditorProvider(
      makeDeps({ getBuildModel: vi.fn().mockReturnValue("ghost-model") }),
    );
    const snapshot = await provider.buildState(PLAN_TEXT, "/p/sample.plan.md");
    expect(snapshot.buildModel).toBe("");
  });

  it("marks canBuild false when the serve lacks set_plan_mode", async () => {
    const provider = new PlanPreviewEditorProvider(
      makeDeps({
        ensureInitialized: vi.fn().mockResolvedValue(initResult(["list_models"])),
      }),
    );
    const snapshot = await provider.buildState(PLAN_TEXT, "/p/sample.plan.md");
    expect(snapshot.canBuild).toBe(false);
    expect(snapshot.availableModels).toEqual(["gpt-5.4"]);
  });

  it("degrades gracefully when initialization fails", async () => {
    const provider = new PlanPreviewEditorProvider(
      makeDeps({
        ensureInitialized: vi.fn().mockRejectedValue(new Error("offline")),
      }),
    );
    const snapshot = await provider.buildState(PLAN_TEXT, "/p/sample.plan.md");
    expect(snapshot.availableModels).toEqual([]);
    expect(snapshot.canBuild).toBe(false);
  });
});

describe("PlanPreviewEditorProvider.handleIntent", () => {
  it("posts a fresh state on plan.ready", async () => {
    const provider = new PlanPreviewEditorProvider(makeDeps());
    const postState = vi.fn().mockResolvedValue(undefined);
    await provider.handleIntent({ messageId: "1", type: "plan.ready" }, makeDoc(), postState);
    expect(postState).toHaveBeenCalledTimes(1);
  });

  it("opens the raw file in the text editor", async () => {
    const deps = makeDeps();
    const provider = new PlanPreviewEditorProvider(deps);
    await provider.handleIntent(
      { messageId: "1", type: "openInTextEditor" },
      makeDoc(PLAN_TEXT, "/workspace/plans/sample.plan.md"),
      vi.fn(),
    );
    expect(deps.openInTextEditor).toHaveBeenCalledWith("/workspace/plans/sample.plan.md");
  });

  it("routes external links to the browser", async () => {
    const deps = makeDeps();
    const provider = new PlanPreviewEditorProvider(deps);
    await provider.handleIntent(
      { data: { href: "https://example.com" }, messageId: "1", type: "openLink" },
      makeDoc(),
      vi.fn(),
    );
    expect(deps.openExternal).toHaveBeenCalledWith("https://example.com");
    expect(deps.openWorkspaceFile).not.toHaveBeenCalled();
  });

  it("routes relative links to the workspace file opener", async () => {
    const deps = makeDeps();
    const provider = new PlanPreviewEditorProvider(deps);
    await provider.handleIntent(
      { data: { href: "docs/design.md" }, messageId: "1", type: "openLink" },
      makeDoc(PLAN_TEXT, "/workspace/plans/sample.plan.md"),
      vi.fn(),
    );
    expect(deps.openWorkspaceFile).toHaveBeenCalledWith(
      path.resolve("/workspace/plans", "docs/design.md"),
    );
  });

  it("falls back to external open when the workspace file cannot be opened", async () => {
    const deps = makeDeps({
      openWorkspaceFile: vi.fn().mockRejectedValue(new Error("missing")),
    });
    const provider = new PlanPreviewEditorProvider(deps);
    await provider.handleIntent(
      { data: { href: "docs/design.md" }, messageId: "1", type: "openLink" },
      makeDoc(PLAN_TEXT, "/workspace/plans/sample.plan.md"),
      vi.fn(),
    );
    expect(deps.openExternal).toHaveBeenCalledWith("docs/design.md");
  });

  it("persists the build model and re-posts state", async () => {
    const deps = makeDeps();
    const provider = new PlanPreviewEditorProvider(deps);
    const postState = vi.fn().mockResolvedValue(undefined);
    await provider.handleIntent(
      { data: { modelId: "gpt-5.4" }, messageId: "1", type: "setBuildModel" },
      makeDoc(),
      postState,
    );
    expect(deps.setBuildModel).toHaveBeenCalledWith("gpt-5.4");
    expect(postState).toHaveBeenCalledTimes(1);
  });

  it("builds the plan using the planId parsed from the document", async () => {
    const deps = makeDeps();
    const provider = new PlanPreviewEditorProvider(deps);
    await provider.handleIntent({ messageId: "1", type: "build" }, makeDoc(), vi.fn());
    expect(deps.buildPlan).toHaveBeenCalledWith("plan-xyz");
  });

  it("forwards an addSelectionToChat intent with its line range", async () => {
    const deps = makeDeps();
    const provider = new PlanPreviewEditorProvider(deps);
    await provider.handleIntent(
      {
        data: { lineEnd: 4, lineStart: 2, text: "selected snippet" },
        messageId: "1",
        type: "addSelectionToChat",
      },
      makeDoc(PLAN_TEXT, "/workspace/plans/sample.plan.md"),
      vi.fn(),
    );
    expect(deps.addSelectionToChat).toHaveBeenCalledWith(
      "/workspace/plans/sample.plan.md",
      "selected snippet",
      { lineEnd: 4, lineStart: 2 },
    );
  });

  it("forwards an addSelectionToChat intent without a line range", async () => {
    const deps = makeDeps();
    const provider = new PlanPreviewEditorProvider(deps);
    await provider.handleIntent(
      { data: { text: "selected snippet" }, messageId: "1", type: "addSelectionToChat" },
      makeDoc(PLAN_TEXT, "/workspace/plans/sample.plan.md"),
      vi.fn(),
    );
    expect(deps.addSelectionToChat).toHaveBeenCalledWith(
      "/workspace/plans/sample.plan.md",
      "selected snippet",
      undefined,
    );
  });
});

describe("PlanPreviewEditorProvider.buildState UI fields", () => {
  it("defaults to preview mode and hybrid toolbar style", async () => {
    const provider = new PlanPreviewEditorProvider(makeDeps());
    const snapshot = await provider.buildState(PLAN_TEXT, "/p/sample.plan.md");
    expect(snapshot.mode).toBe("preview");
    expect(snapshot.toolbarStyle).toBe("hybrid");
  });

  it("passes through host-provided mode and toolbar style", async () => {
    const provider = new PlanPreviewEditorProvider(makeDeps());
    const snapshot = await provider.buildState(PLAN_TEXT, "/p/sample.plan.md", {
      mode: "markdown",
      toolbarStyle: "hybrid",
    });
    expect(snapshot.mode).toBe("markdown");
    expect(snapshot.toolbarStyle).toBe("hybrid");
  });
});

describe("PlanPreviewEditorProvider active-panel + native controls", () => {
  const docPath = "/workspace/plans/sample.plan.md";

  function setup(): {
    events: (PlanActivePanelInfo | null)[];
    provider: PlanPreviewEditorProvider;
    deps: PlanPreviewEditorProviderDeps;
  } {
    __testing.reset();
    const deps = makeDeps();
    const provider = new PlanPreviewEditorProvider(deps);
    const events: (PlanActivePanelInfo | null)[] = [];
    provider.onDidChangeActivePlan((info) => events.push(info));
    return { deps, events, provider };
  }

  it("tracks the focused panel and derives canBuild + mode context", async () => {
    const { provider } = setup();
    await resolveEditor(provider, PLAN_TEXT, docPath);

    const info = provider.getActivePlanInfo();
    expect(info).not.toBeNull();
    expect(info?.path).toBe(docPath);
    expect(info?.mode).toBe("preview");
    expect(info?.canBuild).toBe(true);
  });

  it("merges host mode + toolbarStyle into the posted state frame", async () => {
    const { provider } = setup();
    __testing.setConfiguration("tomcat.plan.toolbarStyle", "hybrid");
    const { panel } = await resolveEditor(provider, PLAN_TEXT, docPath);

    const state = panel.webview.lastState();
    expect(state?.mode).toBe("preview");
    expect(state?.toolbarStyle).toBe("hybrid");
  });

  it("setModeForActive flips the mode, re-posts, and updates context", async () => {
    const { events, provider } = setup();
    const { panel } = await resolveEditor(provider, PLAN_TEXT, docPath);

    await provider.setModeForActive("markdown");
    await flush();

    expect(provider.getActivePlanInfo()?.mode).toBe("markdown");
    expect(panel.webview.lastState()?.mode).toBe("markdown");
    expect(events.at(-1)?.mode).toBe("markdown");
  });

  it("runBuildForActive builds the focused plan's planId", async () => {
    const { deps, provider } = setup();
    await resolveEditor(provider, PLAN_TEXT, docPath);

    await provider.runBuildForActive();
    expect(deps.buildPlan).toHaveBeenCalledWith("plan-xyz");
  });

  it("clears the active plan when the panel loses focus", async () => {
    const { events, provider } = setup();
    const { panel } = await resolveEditor(provider, PLAN_TEXT, docPath);
    expect(provider.getActivePlanInfo()).not.toBeNull();

    panel.setActive(false);
    expect(provider.getActivePlanInfo()).toBeNull();
    expect(events.at(-1)).toBeNull();
  });

  it("does nothing on build/mode when no plan editor is focused", async () => {
    const { deps, provider } = setup();
    await provider.runBuildForActive();
    await provider.setModeForActive("markdown");
    expect(deps.buildPlan).not.toHaveBeenCalled();
    expect(provider.getActivePlanInfo()).toBeNull();
  });

  it("exposes available models for the QuickPick", async () => {
    const { provider } = setup();
    await expect(provider.getAvailableModels()).resolves.toEqual(["gpt-5.4"]);
  });

  it("requestCaptureSelection posts a captureSelectionForChat event to the focused panel", async () => {
    const { provider } = setup();
    const { panel } = await resolveEditor(provider, PLAN_TEXT, docPath);

    await provider.requestCaptureSelection();

    const events = (panel.webview.messages as PlanPreviewHostFrame[]).filter(
      (frame) => frame.channel === "event",
    );
    expect(
      events.some(
        (frame) => (frame.content as { type: string }).type === "captureSelectionForChat",
      ),
    ).toBe(true);
  });

  it("requestCaptureSelection is a no-op when no plan editor is focused", async () => {
    const { provider } = setup();
    await expect(provider.requestCaptureSelection()).resolves.toBeUndefined();
  });
});

describe("PlanPreviewEditorProvider html asset resolution", () => {
  const tempDirs: string[] = [];

  afterEach(async () => {
    await Promise.all(tempDirs.map((dir) => fsPromises.rm(dir, { force: true, recursive: true })));
    tempDirs.length = 0;
  });

  async function createExtensionRoot(files: Record<string, string>): Promise<vscode.Uri> {
    const dir = await fsPromises.mkdtemp(path.join(os.tmpdir(), "tomcat-plan-assets-"));
    tempDirs.push(dir);
    await Promise.all(
      Object.entries(files).map(async ([relativePath, contents]) => {
        const filePath = path.join(dir, relativePath);
        await fsPromises.mkdir(path.dirname(filePath), { recursive: true });
        await fsPromises.writeFile(filePath, contents, "utf8");
      }),
    );
    return vscode.Uri.file(dir);
  }

  it("carries every stylesheet the built plan.html declares (codicon.css guard)", async () => {
    const extensionUri = await createExtensionRoot({
      "gui/dist/plan.html": `<!doctype html><html><head>
        <script type="module" crossorigin src="./plan.js"></script>
        <link rel="stylesheet" crossorigin href="./styles.css">
        <link rel="stylesheet" crossorigin href="./codicon.css">
      </head><body><div id="root"></div></body></html>`,
      "gui/dist/plan.js": "console.log('plan');",
      "gui/dist/styles.css": "body {}",
      "gui/dist/codicon.css": "@font-face { font-family: codicon; }",
    });
    const provider = new PlanPreviewEditorProvider(makeDeps({ extensionUri }));

    const html = (
      provider as unknown as {
        renderHtml(webview: vscode.Webview): string;
      }
    ).renderHtml(new FakeWebview() as unknown as vscode.Webview);

    expect(html).toContain("styles.css");
    expect(html).toContain("codicon.css");
  });
});
