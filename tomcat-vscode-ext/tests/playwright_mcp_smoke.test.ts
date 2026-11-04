import { execFile } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { promisify } from "node:util";
import { expect, it } from "vitest";
import { createPlaywrightMcpSmoke, recordPlaywrightMcpSmoke } from "../scripts/playwrightMcpSmoke";

it("does not accept a scripted completion without a real browser visit", async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), "mcp-evidence-test-"));
  const smoke = await createPlaywrightMcpSmoke();
  try {
    const request = `POST / HTTP/1.1\r\n\r\n${JSON.stringify({ messages: [{ role: "assistant", content: "MCP_BROWSER_SMOKE_COMPLETE" }] })}`;
    await expect(smoke.record([request], directory)).rejects.toThrow("real browser must visit");
  } finally {
    await smoke.close();
    await rm(directory, { recursive: true, force: true });
  }
});

it.each(["valid", "wrong-marker", "missing-image"])("checks snapshot and image when navigation returns a file link: %s", async variant => {
  const directory = await mkdtemp(path.join(os.tmpdir(), "mcp-record-test-"));
  const proof = { marker: "MCP_PROOF_unit_test", url: "http://127.0.0.1:12345", visits: 1 };
  const messages: unknown[] = [
    { role: "tool", tool_call_id: "mcp-navigate", content: `- Page URL: ${proof.url}/\n### Snapshot\n- [Snapshot](page.yml)` },
    { role: "tool", tool_call_id: "mcp-snapshot", content: variant === "wrong-marker" ? "unrelated page" : proof.marker },
    { role: "tool", tool_call_id: "mcp-console", content: "Total messages: 0 (Errors: 0, Warnings: 0)" },
  ];
  if (variant !== "missing-image") messages.push({ role: "user", content: [{ type: "image_url", image_url: { url: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=" } }] });
  const request = `POST / HTTP/1.1\r\n\r\n${JSON.stringify({ messages })}`;
  try {
    const result = recordPlaywrightMcpSmoke([request], proof, directory);
    if (variant === "valid") await expect(result).resolves.toBeUndefined();
    else await expect(result).rejects.toThrow(variant === "wrong-marker" ? "unpredictable page marker" : "PNG image content");
  } finally { await rm(directory, { recursive: true, force: true }); }
});

it("refuses Agent-side MCP acceptance before inspecting or installing a VSIX", async () => {
  await expect(promisify(execFile)(process.execPath, ["--import", "tsx", "scripts/run-vscode-verify-vsix.ts", "--release-bundle", "/missing/must-not-install.vsix"], {
    cwd: path.resolve(__dirname, ".."),
    env: { ...process.env, TOMCAT_AGENT_ACTIVE: "1", TOMCAT_RELEASE_MCP_SMOKE: "1" },
    timeout: 15_000,
  })).rejects.toMatchObject({ code: 1, stderr: expect.stringContaining("Run MCP acceptance from an independent terminal") });
});
