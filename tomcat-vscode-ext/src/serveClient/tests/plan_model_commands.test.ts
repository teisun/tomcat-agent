import { describe, expect, it } from "vitest";

import { TomcatMessenger } from "../TomcatMessenger";
import { createSpawnFactory, FakeChildProcess } from "./fakes";

function readSingleCommandLine(child: FakeChildProcess): Record<string, unknown> {
  const written = child.readStdin().trim();
  expect(written.length).toBeGreaterThan(0);
  return JSON.parse(written);
}

describe("TomcatMessenger plan/model wrappers", () => {
  it("sends set_plan_mode with the expected shape", async () => {
    const child = new FakeChildProcess();
    const messenger = new TomcatMessenger({
      executable: "tomcat",
      spawnFactory: createSpawnFactory(child),
    });

    const pending = messenger.sendSetPlanMode({
      action: "build",
      planId: "plan-42",
      sessionId: "s1",
    });
    const command = readSingleCommandLine(child);

    expect(command).toMatchObject({
      action: "build",
      planId: "plan-42",
      sessionId: "s1",
      type: "set_plan_mode",
    });

    child.emitStdout(
      `${JSON.stringify({
        id: command.id,
        payload: {
          planId: "plan-42",
          planState: "executing",
        },
        sessionId: "s1",
        success: true,
        type: "response",
      })}\n`,
    );

    await expect(pending).resolves.toMatchObject({
      payload: {
        planId: "plan-42",
        planState: "executing",
      },
      sessionId: "s1",
      success: true,
    });
  });

  it("sends list_models with the expected shape", async () => {
    const child = new FakeChildProcess();
    const messenger = new TomcatMessenger({
      executable: "tomcat",
      spawnFactory: createSpawnFactory(child),
    });

    const pending = messenger.sendListModels();
    const command = readSingleCommandLine(child);

    expect(command).toMatchObject({
      type: "list_models",
    });

    child.emitStdout(
      `${JSON.stringify({
        id: command.id,
        payload: {
          models: [{ id: "gpt-5.4" }, { id: "claude-opus-4" }],
        },
        success: true,
        type: "response",
      })}\n`,
    );

    await expect(pending).resolves.toMatchObject({
      payload: {
        models: [{ id: "gpt-5.4" }, { id: "claude-opus-4" }],
      },
      success: true,
    });
  });
});
