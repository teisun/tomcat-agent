import { test, expect } from "@playwright/test";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { TestApiClient } from "./fixtures";

// Opt-in: a real daemon and explicitly built Tomcat, but only a private database,
// HOME and loopback provider. No route mocks and no installed agent executable.
test("Tomcat model discovery, existing error feedback, save and actual execution", async ({ page }) => {
  const root = process.env.TOMCAT_DISCOVERY_FIXTURE_ROOT;
  test.skip(!root, "requires the isolated Tomcat catalog fixture");
  test.setTimeout(450_000);
  page.setDefaultTimeout(30_000);
  page.setDefaultNavigationTimeout(90_000);
  expect(root).toMatch(/^\/private\/tmp\/tcm-[\w-]+$/);
  const fixtureEnv = JSON.parse(readFileSync(path.join(root!, "env.json"), "utf8"));
  expect(fixtureEnv.HOME).toBe(path.join(root!, "home"));
  const bin = fixtureEnv.MULTICA_TOMCAT_PATH;
  expect(path.isAbsolute(bin)).toBe(true);
  const out = process.env.TOMCAT_DISCOVERY_EVIDENCE!;
  expect(path.isAbsolute(out)).toBe(true);
  mkdirSync(out, { recursive: true });
  const shots = process.env.TOMCAT_DISCOVERY_SHOTS ?? out;
  mkdirSync(shots, { recursive: true });
  const consoleReport: { errors: string[]; warnings: string[]; pageErrors: string[] } = { errors: [], warnings: [], pageErrors: [] };
  page.on("console", (msg) => {
    if (msg.type() === "error") consoleReport.errors.push(msg.text());
    if (msg.type() === "warning") consoleReport.warnings.push(msg.text());
  });
  page.on("pageerror", (error) => consoleReport.pageErrors.push(error.message));
  const assets: { kind: string; path: string; status: number }[] = [];
  const reports: unknown[] = [];
  page.on("response", async (res) => {
    const kind = res.request().resourceType();
    if (["script", "stylesheet", "font"].includes(kind)) {
      assets.push({ kind, path: new URL(res.url()).pathname, status: res.status() });
    }
    if (/\/api\/runtimes\/[^/]+\/models(?:\?|$)/.test(res.url())) {
      try { reports.push({ status: res.status(), body: await res.json() }); } catch { /* aborted polling */ }
    }
  });
  async function capture(name: string) {
    await page.screenshot({ path: path.join(shots, `${name}.png`) });
    writeFileSync(path.join(out, `${name}.aria.txt`), await page.locator("body").ariaSnapshot());
    writeFileSync(path.join(out, `${name}.console.json`), JSON.stringify(consoleReport, null, 2));
  }
  const api = new TestApiClient();
  await api.login(process.env.MULTICA_DEV_EMAIL!, "Catalog fixture");
  const ws = await api.ensureWorkspace("Catalog fixture", "catalog-fixture");
  await api.markUserOnboarded();
  const apiURL = process.env.NEXT_PUBLIC_API_URL!;
  async function request(endpoint: string, method = "GET", body?: unknown) {
    const res = await fetch(`${apiURL}${endpoint}`, { method, headers: {
      Authorization: `Bearer ${api.getToken()}`, "Content-Type": "application/json", "X-Workspace-ID": ws.id,
    }, body: body === undefined ? undefined : JSON.stringify(body) });
    expect(res.ok, `${method} ${endpoint}: ${res.status}`).toBe(true);
    return res.status === 204 ? null : res.json();
  }
  await page.addInitScript((token) => {
    localStorage.setItem("multica_token", token!);
    localStorage.setItem("multica:chat:isOpen", "false");
  }, api.getToken());
  let runtime: any;
  await expect.poll(async () => {
    runtime = (await request("/api/runtimes?owner=me")).find((r: any) => r.provider === "tomcat" && !r.profile_id);
    return runtime?.status;
  }, { timeout: 45_000 }).toBe("online");
  const manual = (runtimeID: string) => `/${ws.slug}/agents/new/manual?runtime=${runtimeID}`;
  const source = JSON.parse(readFileSync(path.join(process.cwd(), "packages/views/locales/en/agents.json"), "utf8"));
  const title = source.pickers.model_discovery_failed_title;
  const hint = source.pickers.model_discovery_failed_hint;
  const search = () => page.getByRole("textbox", { name: source.pickers.model_search_placeholder });
  const defaultTrigger = () => page.getByRole("button", { name: source.model_dropdown.default_provider, exact: true });
  const refresh = () => page.getByRole("button", { name: source.pickers.model_refresh, exact: true });
  const thinkingTrigger = (label: string) => page.getByRole("button", { name: source.pickers.thinking_tooltip.replace("{{value}}", label), exact: true });
  const modelsPath = path.join(out, "provider-models.jsonl");
  let createdID: string | undefined;
  try {
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.goto(manual(runtime.id), { waitUntil: "domcontentloaded" });
    await defaultTrigger().click();
    await expect(page.getByText(title, { exact: true })).toBeVisible({ timeout: 45_000 });
    await expect(page.getByText(/Tomcat has no models with saved credentials/)).toBeVisible();
    await expect(page.getByText(hint, { exact: true })).toBeVisible();
    expect(existsSync(modelsPath) ? readFileSync(modelsPath, "utf8").trim() : "").toBe("");
    await capture("no-credentials-desktop");
    await page.setViewportSize({ width: 390, height: 844 });
    await capture("no-credentials-narrow");

    // Save to exactly the runtime HOME. An explicit refresh must work without
    // restarting either daemon or Tomcat, and without a new adapter memo.
    execFileSync(bin, ["model", "key", "set", "catalog-fixture", "local-fixture-key"], { env: fixtureEnv, cwd: root, stdio: "pipe" });
    await refresh().click();
    await expect(page.getByRole("button", { name: "catalog-alias", exact: true })).toBeVisible({ timeout: 45_000 });
    await expect(page.getByRole("button", { name: "catalog-default", exact: true })).toBeVisible();
    await expect(page.getByText("no-key-model", { exact: true })).toHaveCount(0);
    await expect(page.getByText(title, { exact: true })).toHaveCount(0);
    expect(existsSync(modelsPath) ? readFileSync(modelsPath, "utf8").trim() : "").toBe("");
    await capture("catalog-narrow");
    await page.setViewportSize({ width: 1440, height: 1000 });
    await capture("catalog-desktop");
    await page.getByRole("button", { name: "catalog-no-effort", exact: true }).click();
    await expect(thinkingTrigger(source.pickers.thinking_default)).toHaveCount(0);
    await expect(search()).toHaveCount(0);
    // The selected trigger includes a second descriptive line, unlike its menu item.
    await page.getByRole("button", { name: /catalog-no-effort/ }).click();
    await page.getByRole("button", { name: "catalog-alias", exact: true }).click();
    await thinkingTrigger(source.pickers.thinking_default).click();
    await expect(page.getByRole("button", { name: "High", exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "Low", exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "Medium", exact: true })).toHaveCount(0);
    await capture("effort-menu-desktop");
    await page.getByRole("button", { name: "High", exact: true }).click();
    await page.setViewportSize({ width: 390, height: 844 });
    await thinkingTrigger("High").scrollIntoViewIfNeeded();
    await thinkingTrigger("High").click();
    await expect(page.getByRole("button", { name: "Low", exact: true })).toBeInViewport();
    await expect(page.getByRole("button", { name: "High", exact: true })).toBeInViewport();
    await capture("effort-menu-narrow");
    await page.getByRole("button", { name: "High", exact: true }).click();
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.locator("#agent-create-name").fill("Catalog discovered agent");
    await page.locator("#agent-create-instructions").fill("Reply with catalog-fixture-done. Do not call tools.");
    const committed = page.waitForResponse((res) => res.request().method() === "POST" && /\/api\/agents$/.test(res.url()));
    await page.getByRole("button", { name: source.creation_studio.create_agent, exact: true }).click();
    const response = await committed;
    expect(response.ok()).toBe(true);
    const created = await response.json();
    createdID = created.id;
    expect(created.model).toBe("catalog-alias");
    expect(created.thinking_level).toBe("high");
    const saved = await request(`/api/agents/${createdID}`);
    expect(saved.model).toBe("catalog-alias");
    expect(saved.thinking_level).toBe("high");
    // Let the create flow finish its own confirmation navigation before opening
    // another route; racing router.push with a hard navigation cold-compiles both.
    await page.waitForURL(`**/agents/new/done/${createdID}`, { timeout: 90_000 });
    // Use the UI-created agent. The server constructs the task payload, the
    // real daemon claims it, and the real ACP adapter must use the saved ID.
    const issue = await api.createIssue("Catalog fixture task", { description: "Reply with catalog-fixture-done.", assignee_type: "agent", assignee_id: createdID, status: "todo" });
    const runs: any[] = [];
    async function completed() {
      let next: any;
      await expect.poll(async () => {
        next = (await request(`/api/issues/${issue.id}/task-runs`)).find((candidate: any) => candidate.id !== undefined && !runs.some((old) => old.id === candidate.id));
        if (next && ["failed", "cancelled", "timeout", "aborted"].includes(next.status)) throw new Error(`fixture task ${next.status}: ${next.error}`);
        return next?.status;
      }, { timeout: 90_000, intervals: [1000] }).toBe("completed");
      expect(next.result.output).toContain("catalog-fixture-done");
      runs.push(next);
      return next;
    }
    const run = await completed();
    expect(run.agent_id).toBe(createdID);
    expect(run.runtime_id).toBe(runtime.id);
    expect(run.result.output).toContain("catalog-fixture-done");
    writeFileSync(path.join(out, "initial-task.json"), JSON.stringify({ runtimeID: runtime.id, agentID: createdID, savedModel: saved.model, savedEffort: saved.thinking_level, issueID: issue.id, run }, null, 2));
    await page.goto(`/${ws.slug}/agents/${createdID}`, { waitUntil: "domcontentloaded" });
    await expect(page.getByText("catalog-alias", { exact: true }).first()).toBeVisible({ timeout: 45_000 });
    await capture("saved-agent-desktop");
    await page.getByRole("button", { name: source.detail.settings, exact: true }).click();
    await expect(page).toHaveURL(/\bview=general(?:&|$)/);
    await expect(thinkingTrigger("High")).toBeVisible();
    await thinkingTrigger("High").scrollIntoViewIfNeeded();
    await capture("saved-effort-high-desktop");
    for (const [before, label, expected] of [["High", "Low", "low"], ["Low", source.pickers.thinking_clear, ""]]) {
      await thinkingTrigger(before).click();
      const changed = page.waitForResponse((res) => ["PUT", "PATCH"].includes(res.request().method()) && res.url().endsWith(`/api/agents/${createdID}`));
      await page.getByRole("button", { name: label, exact: true }).click();
      expect((await changed).ok()).toBe(true);
      expect((await request(`/api/agents/${createdID}`)).thinking_level ?? "").toBe(expected);
      await page.reload({ waitUntil: "domcontentloaded" });
      // Empty/default hides the row until runtime-model discovery finishes after reload.
      await expect(thinkingTrigger(label)).toBeVisible({ timeout: 45_000 });
      await thinkingTrigger(label).scrollIntoViewIfNeeded();
      if (expected === "low") await capture("effort-low-desktop");
      await api.createComment(issue.id, "Reply again with catalog-fixture-done.");
      const resumed = await completed();
      expect(resumed.result.session_id).toBe(run.result.session_id);
    }
    expect(run.result.session_id).toBeTruthy();
    await capture("effort-default-desktop");
    await page.setViewportSize({ width: 390, height: 844 });
    await thinkingTrigger(source.pickers.thinking_default).scrollIntoViewIfNeeded();
    await capture("effort-default-narrow");
    await page.setViewportSize({ width: 1440, height: 1000 });
    const wires = readFileSync(modelsPath, "utf8").trim().split("\n").map((line) => JSON.parse(line));
    expect(wires.some((r) => r.model === "alias-wire" && r.stream === true)).toBe(true);
    expect(wires.every((r) => ["alias-wire", "default-wire"].includes(r.model))).toBe(true);
    expect(wires.filter((r) => r.stream === true && !r.title).map((r) => r.reasoning_effort)).toEqual(["high", "low", "high"]);
    writeFileSync(path.join(out, "journey.json"), JSON.stringify({ runtimeID: runtime.id, agentID: createdID, savedModel: saved.model, issueID: issue.id, taskID: run.id, taskStatus: run.status, resumedRuns: runs, providerModels: wires }, null, 2));

    const legacy = await request(`/api/workspaces/${ws.id}/runtime-profiles`, "POST", {
      display_name: "Catalog legacy fixture", runtime_type: "tomcat", protocol_family: "tomcat", command_name: path.join(root!, "bin/catalog-legacy"), fixed_args: [], enabled: true,
    });
    let oldRuntime: any;
    await expect.poll(async () => {
      oldRuntime = (await request("/api/runtimes?owner=me")).find((r: any) => r.profile_id === legacy.id);
      return oldRuntime?.status;
    }, { timeout: 150_000 }).toBe("online");
    await page.goto(manual(oldRuntime.id), { waitUntil: "domcontentloaded" });
    await defaultTrigger().click();
    await expect(page.getByText(title, { exact: true })).toBeVisible({ timeout: 45_000 });
    await expect(page.getByText(/unrecognized subcommand models/)).toBeVisible();
    await expect(page.getByText(hint, { exact: true })).toBeVisible();
    await capture("old-command-desktop");
    await page.setViewportSize({ width: 390, height: 844 });
    await capture("old-command-narrow");
    await search().fill("manual-model-id");
    await page.getByRole("button", { name: 'Use "manual-model-id"', exact: true }).click();
    await expect(page.getByRole("button", { name: /manual-model-id custom/ })).toBeVisible();
    // Default remains a real choice, not an automatically selected first model.
    await page.getByRole("button", { name: /manual-model-id custom/ }).click();
    await page.getByRole("button", { name: source.model_dropdown.clear_full, exact: true }).click();
    await expect(defaultTrigger()).toBeVisible();
    await request(`/api/workspaces/${ws.id}/runtime-profiles/${legacy.id}`, "DELETE");
  } catch (error) {
    // Capture before cleanup can archive the agent and replace the failed page.
    try { await capture("journey-failed"); }
    catch (captureError) { consoleReport.warnings.push(`Failure capture: ${String(captureError)}`); }
    throw error;
  } finally {
    writeFileSync(path.join(out, "models-responses.json"), JSON.stringify(reports, null, 2));
    writeFileSync(path.join(out, "console.json"), JSON.stringify(consoleReport, null, 2));
    writeFileSync(path.join(out, "assets.json"), JSON.stringify(assets, null, 2));
    await api.cleanup();
    if (createdID) await request(`/api/agents/${createdID}/archive`, "POST");
  }
  expect(consoleReport.pageErrors).toEqual([]);
  expect(consoleReport.errors).toEqual([]);
});
