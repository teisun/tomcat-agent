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
  expect(root).toMatch(/^\/private\/tmp\/tcm-[\w-]+$/);
  const fixtureEnv = JSON.parse(readFileSync(path.join(root!, "env.json"), "utf8"));
  expect(fixtureEnv.HOME).toBe(path.join(root!, "home"));
  const bin = fixtureEnv.MULTICA_TOMCAT_PATH;
  expect(path.isAbsolute(bin)).toBe(true);
  const out = process.env.TOMCAT_DISCOVERY_EVIDENCE!;
  expect(path.isAbsolute(out)).toBe(true);
  mkdirSync(out, { recursive: true });
  const consoleReport: { errors: string[]; warnings: string[]; pageErrors: string[] } = { errors: [], warnings: [], pageErrors: [] };
  page.on("console", (msg) => {
    if (msg.type() === "error") consoleReport.errors.push(msg.text());
    if (msg.type() === "warning") consoleReport.warnings.push(msg.text());
  });
  page.on("pageerror", (error) => consoleReport.pageErrors.push(error.message));
  const reports: unknown[] = [];
  page.on("response", async (res) => {
    if (/\/api\/runtimes\/[^/]+\/models(?:\?|$)/.test(res.url())) {
      try { reports.push({ status: res.status(), body: await res.json() }); } catch { /* aborted polling */ }
    }
  });
  async function capture(name: string) {
    await page.screenshot({ path: path.join(out, `${name}.png`) });
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
    await page.getByRole("button", { name: "catalog-alias", exact: true }).click();
    await page.locator("#agent-create-name").fill("Catalog discovered agent");
    await page.locator("#agent-create-instructions").fill("Reply with catalog-fixture-done. Do not call tools.");
    const committed = page.waitForResponse((res) => res.request().method() === "POST" && /\/api\/agents$/.test(res.url()));
    await page.getByRole("button", { name: source.creation_studio.create_agent, exact: true }).click();
    const response = await committed;
    expect(response.ok()).toBe(true);
    const created = await response.json();
    createdID = created.id;
    expect(created.model).toBe("catalog-alias");
    const saved = await request(`/api/agents/${createdID}`);
    expect(saved.model).toBe("catalog-alias");
    await page.goto(`/${ws.slug}/agents/${createdID}`, { waitUntil: "domcontentloaded" });
    await expect(page.getByText("catalog-alias", { exact: true }).first()).toBeVisible({ timeout: 45_000 });
    await capture("saved-agent-desktop");
    // Use the UI-created agent. The server constructs the task payload, the
    // real daemon claims it, and the real ACP adapter must use the saved ID.
    const issue = await api.createIssue("Catalog fixture task", { description: "Reply with catalog-fixture-done.", assignee_type: "agent", assignee_id: createdID, status: "todo" });
    let run: any;
    await expect.poll(async () => {
      const runs = await request(`/api/issues/${issue.id}/task-runs`);
      run = runs[0];
      if (run && ["failed", "cancelled", "timeout", "aborted"].includes(run.status)) throw new Error(`fixture task ${run.status}: ${run.error}`);
      return run?.status;
    }, { timeout: 90_000, intervals: [1000] }).toBe("completed");
    expect(run.agent_id).toBe(createdID);
    expect(run.runtime_id).toBe(runtime.id);
    expect(run.result.output).toContain("catalog-fixture-done");
    const wires = readFileSync(modelsPath, "utf8").trim().split("\n").map((line) => JSON.parse(line));
    expect(wires.some((r) => r.model === "alias-wire" && r.stream === true)).toBe(true);
    expect(wires.every((r) => ["alias-wire", "default-wire"].includes(r.model))).toBe(true);
    writeFileSync(path.join(out, "journey.json"), JSON.stringify({ runtimeID: runtime.id, agentID: createdID, savedModel: saved.model, issueID: issue.id, taskID: run.id, taskStatus: run.status, providerModels: wires }, null, 2));

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
  } finally {
    writeFileSync(path.join(out, "models-responses.json"), JSON.stringify(reports, null, 2));
    writeFileSync(path.join(out, "console.json"), JSON.stringify(consoleReport, null, 2));
    await api.cleanup();
    if (createdID) await request(`/api/agents/${createdID}/archive`, "POST");
  }
  expect(consoleReport.pageErrors).toEqual([]);
  expect(consoleReport.errors).toEqual([]);
});
