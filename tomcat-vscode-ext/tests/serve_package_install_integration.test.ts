import { access, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { initializeServe } from "../src/serveClient/initialize";
import {
  createRealServeMessenger,
  responsesCompleted,
  responsesFunctionCallAdded,
  responsesFunctionCallArgumentsDelta,
  responsesTextDelta,
  spawnScriptedOpenAiStreamServer,
  waitForEvent,
  warmTomcatBinaryForSuite,
} from "./serveTestUtils";

warmTomcatBinaryForSuite(300_000);

describe("real tomcat serve package_install integration", () => {
  it("round-trips the native confirmation, writes the target, and audits the install", async () => {
    const sourceRoot = await mkdtemp(join(tmpdir(), "tomcat-serve-package-"));
    const source = join(sourceRoot, "serve-package-source");
    await mkdir(source, { recursive: true });
    await writeFile(
      join(source, "SKILL.md"),
      "---\nname: serve-package\ndescription: serve integration fixture\n---\n# Serve package\n",
      "utf8",
    );
    const server = await spawnScriptedOpenAiStreamServer([
      {
        parts: [
          responsesFunctionCallAdded("package_call", "call_package", "package_install"),
          responsesFunctionCallArgumentsDelta("package_call", JSON.stringify({
            source,
            scope: "agent",
          })),
          responsesCompleted(),
        ],
      },
      {
        parts: [responsesTextDelta("package installed after confirmation"), responsesCompleted()],
      },
    ]);
    const runtime = await createRealServeMessenger(server.baseUrl, "openai-responses");

    try {
      runtime.fixture.env.TOMCAT__SECURITY__ENABLE_AUDIT_LOG = "true";

      const confirmationOperations: string[] = [];
      runtime.messenger.registerControlRequestHandler("confirmation", (frame) => {
        const payload = frame.payload as { operation?: unknown };
        confirmationOperations.push(String(payload.operation));
        expect(frame.sessionId).toBeTruthy();
        return {
          kind: "response",
          payload: { decision: "allow_once" },
          sessionId: frame.sessionId,
        };
      });

      const init = await initializeServe(runtime.messenger);
      const agentEnd = waitForEvent(
        runtime.messenger,
        (event) => event.type === "agent_end",
        30_000,
      );
      await runtime.messenger.request({
        params: {},
        sessionId: init.sessionId,
        text: "install the package",
        type: "prompt",
      });
      const events = await agentEnd;

      expect(confirmationOperations).toEqual(["Write"]);
      expect(
        events.some(
          (event) =>
            event.type === "message_update" &&
            (event.assistantMessageEvent as { delta?: string }).delta ===
              "package installed after confirmation",
        ),
      ).toBe(true);
      await expect(
        access(join(runtime.fixture.homePath, ".tomcat", "agents", "main", "skills", "serve-package", "SKILL.md")),
      ).resolves.toBeUndefined();
      const audit = await readFile(
        join(runtime.fixture.homePath, ".tomcat", "agents", "main", "audit", "audit.jsonl"),
        "utf8",
      );
      expect(audit).toContain('"tool_name":"package_install"');
      expect(audit).toContain('\\"status\\":\\"approved\\"');
      expect(audit).toContain('\\"status\\":\\"installed\\"');
      expect(server.capturedNonTitleRequests()).toHaveLength(2);
    } finally {
      await runtime.cleanup();
      await server.close();
      await rm(sourceRoot, { force: true, recursive: true });
    }
  }, 45_000);
});
