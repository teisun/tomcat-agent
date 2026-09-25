import { describe, expect, it, vi } from "vitest";

import { ProjectTrustPrompt } from "./ProjectTrustPrompt";

const ready = { capabilities: ["get_project_trust", "trust_project"] };
const response = (root: string, trusted = false, error?: string) => ({
  success: true,
  payload: { projectRoot: root, trusted, ...(error ? { error } : {}) },
});

function fixture() {
  let cwd: string | undefined = "/workspace/one";
  const messenger = {
    sendGetProjectTrust: vi.fn(async (_cwd: string) => response("/workspace/one")),
    sendTrustProject: vi.fn(async (root: string) => response(root, true)),
  };
  const confirm = vi.fn(async () => true);
  const reportError = vi.fn();
  const prompt = new ProjectTrustPrompt({
    getDefaultCwd: () => cwd,
    messenger: messenger as unknown as ConstructorParameters<typeof ProjectTrustPrompt>[0]["messenger"],
    confirm,
    reportError,
  });
  return { prompt, messenger, confirm, reportError, setCwd: (value?: string) => { cwd = value; } };
}

describe("ProjectTrustPrompt", () => {
  it("asks at readiness without opening Settings, persists only the project root and notifies", async () => {
    const h = fixture();
    const listener = vi.fn();
    h.prompt.onDidTrustProject(listener);
    await h.prompt.onServeReady(ready);
    expect(h.confirm).toHaveBeenCalledExactlyOnceWith("/workspace/one");
    expect(h.messenger.sendTrustProject).toHaveBeenCalledExactlyOnceWith("/workspace/one");
    expect(listener).toHaveBeenCalledTimes(1);
    await h.prompt.onServeReady(ready);
    expect(h.confirm).toHaveBeenCalledTimes(1);
  });

  it("Not now does not grant or persist denial, but a different project is asked", async () => {
    const h = fixture();
    h.confirm.mockResolvedValue(false);
    await h.prompt.onServeReady(ready);
    await h.prompt.onServeReady(ready);
    expect(h.confirm).toHaveBeenCalledTimes(1);
    expect(h.messenger.sendTrustProject).not.toHaveBeenCalled();
    h.setCwd("/workspace/two");
    h.messenger.sendGetProjectTrust.mockResolvedValue(response("/workspace/two"));
    await h.prompt.onServeReady(ready);
    expect(h.confirm).toHaveBeenCalledTimes(2);
  });

  it("skips absent cwd/capabilities and unreadable trust record without guessing", async () => {
    const h = fixture();
    h.setCwd(undefined);
    await h.prompt.onServeReady(ready);
    h.setCwd("/workspace/one");
    await h.prompt.onServeReady({ capabilities: [] });
    expect(h.messenger.sendGetProjectTrust).not.toHaveBeenCalled();
    h.messenger.sendGetProjectTrust.mockResolvedValue(response("/workspace/one", false, "corrupt record"));
    await h.prompt.onServeReady(ready);
    expect(h.reportError).toHaveBeenCalledWith(expect.any(Error), false);
    expect(h.confirm).not.toHaveBeenCalled();
    expect(h.messenger.sendTrustProject).not.toHaveBeenCalled();
  });

  it("does not trust a stale modal after the active project changes", async () => {
    const h = fixture();
    h.messenger.sendGetProjectTrust.mockImplementation(async (cwd: string) => response(cwd));
    h.confirm.mockImplementation(async () => { h.setCwd("/workspace/two"); return true; });
    await h.prompt.onServeReady(ready);
    expect(h.messenger.sendTrustProject).not.toHaveBeenCalled();
    expect(h.reportError).not.toHaveBeenCalled();
  });

  it("coalesces parallel ready notifications and rejects a mismatched receipt", async () => {
    const h = fixture();
    let release!: (value: boolean) => void;
    h.confirm.mockImplementation(() => new Promise<boolean>((resolve) => { release = resolve; }));
    const first = h.prompt.onServeReady(ready);
    await vi.waitFor(() => expect(h.confirm).toHaveBeenCalledOnce());
    await h.prompt.onServeReady(ready);
    expect(h.confirm).toHaveBeenCalledOnce();
    h.messenger.sendTrustProject.mockResolvedValue(response("/workspace/other", true));
    release(true);
    await first;
    expect(h.reportError).toHaveBeenCalledWith(expect.any(Error), true);
  });
});
