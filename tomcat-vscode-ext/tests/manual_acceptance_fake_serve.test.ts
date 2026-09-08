import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";

import { afterEach, describe, expect, it } from "vitest";

type ResponseFrame = {
  error?: string;
  id?: string;
  payload?: unknown;
  requestId?: string;
  sessionId?: string;
  success?: boolean;
  type?: string;
};

class ManualAcceptanceFakeServe {
  private readonly pending = new Map<
    string,
    {
      reject: (error: Error) => void;
      resolve: (frame: ResponseFrame) => void;
      timer: NodeJS.Timeout;
    }
  >();
  private stderr = "";

  constructor(
    private readonly child: ChildProcessWithoutNullStreams,
    private readonly stateDir: string,
  ) {
    let stdoutBuffer = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      stdoutBuffer += chunk;
      const lines = stdoutBuffer.split("\n");
      stdoutBuffer = lines.pop() ?? "";
      for (const line of lines) {
        if (!line.trim()) continue;
        const frame = JSON.parse(line) as ResponseFrame;
        const requestId = frame.id ?? frame.requestId;
        if (!requestId) continue;
        const pending = this.pending.get(requestId);
        if (!pending) continue;
        this.pending.delete(requestId);
        clearTimeout(pending.timer);
        pending.resolve(frame);
      }
    });
    child.stderr.on("data", (chunk: string) => {
      this.stderr += chunk;
    });
  }

  request(
    requestId: string,
    frame: Record<string, unknown>,
  ): Promise<ResponseFrame> {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(requestId);
        reject(
          new Error(
            `fake serve did not answer ${requestId}; stderr:\n${this.stderr}`,
          ),
        );
      }, 5_000);
      this.pending.set(requestId, { reject, resolve, timer });
      this.child.stdin.write(`${JSON.stringify(frame)}\n`);
    });
  }

  async close(): Promise<void> {
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(new Error("fake serve closed before responding"));
    }
    this.pending.clear();
    if (this.child.exitCode === null && !this.child.killed) {
      await new Promise<void>((resolve) => {
        this.child.once("exit", () => resolve());
        this.child.kill();
      });
    }
    await rm(this.stateDir, { force: true, recursive: true });
  }
}

const runningFakes: ManualAcceptanceFakeServe[] = [];

afterEach(async () => {
  await Promise.all(runningFakes.splice(0).map((fake) => fake.close()));
});

async function startFakeServe(): Promise<ManualAcceptanceFakeServe> {
  const stateDir = await mkdtemp(path.join(os.tmpdir(), "tomcat-fake-serve-test-"));
  const fixturePath = path.resolve(
    __dirname,
    "../scripts/manual-acceptance/fake-serve.js",
  );
  const child = spawn(process.execPath, [fixturePath], {
    env: {
      ...process.env,
      TOMCAT_FAKE_SERVE_STATE_DIR: stateDir,
    },
    stdio: ["pipe", "pipe", "pipe"],
  });
  const fake = new ManualAcceptanceFakeServe(child, stateDir);
  runningFakes.push(fake);
  return fake;
}

describe("manual acceptance fake serve attachment leases", () => {
  it("advertises and implements the retain_attachment_leases wire contract", async () => {
    const fake = await startFakeServe();
    const initialized = await fake.request("initialize", {
      requestId: "initialize",
      subtype: "initialize",
      type: "control_request",
    });
    const initializePayload = initialized.payload as {
      capabilities: string[];
      protocolVersion: number;
      sessionId: string;
    };
    expect(initializePayload.protocolVersion).toBe(2);
    expect(initializePayload.capabilities).toContain("retain_attachment_leases");
    expect(initializePayload.capabilities).toContain("list_checkpoints");
    const checkpoints = await fake.request("checkpoints", {
      id: "checkpoints",
      sessionId: initializePayload.sessionId,
      type: "list_checkpoints",
    });
    expect(checkpoints).toMatchObject({
      success: true,
      payload: { checkpoints: [], sessionId: initializePayload.sessionId },
    });
    const state = await fake.request("state", {
      id: "state", sessionId: initializePayload.sessionId, type: "get_state",
    });
    expect(state.payload).toMatchObject({ agentMode: "chat", workspaceMode: "project" });

    const ingested = await fake.request("ingest", {
      attachment: {
        dataBase64: Buffer.from("attachment bytes").toString("base64"),
        filename: "proof.png",
        mimeType: "image/png",
      },
      id: "ingest",
      sessionId: initializePayload.sessionId,
      type: "ingest_attachment",
    });
    expect(ingested.success).toBe(true);
    const blobSha = (ingested.payload as { blobSha: string }).blobSha;

    const retained = await fake.request("retain", {
      id: "retain",
      params: {
        attachments: [{ blobSha }, { blobSha, providerSha: blobSha }],
      },
      sessionId: initializePayload.sessionId,
      type: "retain_attachment_leases",
    });
    expect(retained).toMatchObject({
      payload: { retainedShas: [blobSha] },
      sessionId: initializePayload.sessionId,
      success: true,
    });

    const prompted = await fake.request("prompt-with-file", {
      id: "prompt-with-file",
      params: {
        attachments: [
          {
            blobSha,
            filename: "proof.pdf",
            kind: "file",
            mimeType: "application/pdf",
          },
        ],
      },
      sessionId: initializePayload.sessionId,
      text: "retain this file reference in history",
      type: "prompt",
    });
    expect(prompted.success).toBe(true);
    const history = await fake.request("get-messages", {
      id: "get-messages",
      sessionId: initializePayload.sessionId,
      type: "get_messages",
    });
    const userMessage = (
      (history.payload as {
        messages: Array<{ message: { content: unknown; role: string } }>;
      }).messages
    ).find((entry) => entry.message.role === "user" && Array.isArray(entry.message.content));
    expect(userMessage?.message.content).toEqual([
      { text: "retain this file reference in history", type: "input_text" },
      {
        blobSha,
        bytes: "attachment bytes".length,
        filename: "proof.pdf",
        mime_type: "application/pdf",
        type: "input_file",
      },
    ]);

    const missing = await fake.request("retain-missing", {
      id: "retain-missing",
      params: {
        attachments: [{ blobSha: "0".repeat(64) }],
      },
      sessionId: initializePayload.sessionId,
      type: "retain_attachment_leases",
    });
    expect(missing).toMatchObject({
      error: `missing_attachment_blob: ${"0".repeat(64)}`,
      success: false,
    });
  });
});
