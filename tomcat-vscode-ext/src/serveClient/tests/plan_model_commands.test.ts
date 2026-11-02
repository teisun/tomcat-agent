import { describe, expect, it } from "vitest";

import { createReadyMessenger } from "./fakes";
import { createConnectionFactory, FakeConnection } from "./fakes";

function readSingleCommandLine(child: FakeConnection): Record<string, unknown> {
  const written = child.readStdin().trim();
  expect(written.length).toBeGreaterThan(0);
  return JSON.parse(written);
}

describe("TomcatMessenger plan/model wrappers", () => {
  it("sends set_plan_mode with the expected shape", async () => {
    const child = new FakeConnection();
    const messenger = await createReadyMessenger({
      executable: "tomcat",
      connectFactory: createConnectionFactory(child),
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
    const child = new FakeConnection();
    const messenger = await createReadyMessenger({
      executable: "tomcat",
      connectFactory: createConnectionFactory(child),
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

  it("sends model admin commands with the expected payloads", async () => {
    const child = new FakeConnection();
    const messenger = await createReadyMessenger({
      executable: "tomcat",
      connectFactory: createConnectionFactory(child),
    });

    const pendingUpsert = messenger.sendUpsertModel({
      api: "anthropic-messages",
      apiKeyEnv: "ANTHROPIC_API_KEY",
      baseUrl: "https://api.anthropic.com/v1",
      capabilities: {
        reasoning: true,
        tools: true,
      },
      contextWindow: 200_000,
      id: "claude-opus-gateway",
      modelName: "claude-opus-4-6",
      provider: "anthropic",
      thinkingFormat: "anthropic",
    });
    const upsert = readSingleCommandLine(child);
    expect(upsert).toMatchObject({
      model: {
        api: "anthropic-messages",
        apiKeyEnv: "ANTHROPIC_API_KEY",
        id: "claude-opus-gateway",
        provider: "anthropic",
      },
      type: "upsert_model",
    });
    child.emitStdout(
      `${JSON.stringify({
        id: upsert.id,
        payload: {
          model: {
            id: "claude-opus-gateway",
          },
        },
        success: true,
        type: "response",
      })}\n`,
    );
    await expect(pendingUpsert).resolves.toMatchObject({ success: true });

    const pendingSetKey = messenger.sendSetProviderKey("ANTHROPIC_API_KEY", "secret");
    const setKey = readSingleCommandLine(child);
    expect(setKey).toMatchObject({
      envName: "ANTHROPIC_API_KEY",
      type: "set_provider_key",
      value: "secret",
    });
    child.emitStdout(
      `${JSON.stringify({
        id: setKey.id,
        payload: { envName: "ANTHROPIC_API_KEY", keyPresent: true },
        success: true,
        type: "response",
      })}\n`,
    );
    await expect(pendingSetKey).resolves.toMatchObject({ success: true });

    const pendingListKeys = messenger.sendListProviderKeys();
    const listKeys = readSingleCommandLine(child);
    expect(listKeys).toMatchObject({
      type: "list_provider_keys",
    });
    child.emitStdout(
      `${JSON.stringify({
        id: listKeys.id,
        payload: {
          keys: [{ envName: "ANTHROPIC_API_KEY", keyPresent: true }],
        },
        success: true,
        type: "response",
      })}\n`,
    );
    await expect(pendingListKeys).resolves.toMatchObject({ success: true });

    const pendingDefault = messenger.sendSetDefaultModel("claude-opus-gateway");
    const setDefault = readSingleCommandLine(child);
    expect(setDefault).toMatchObject({
      modelId: "claude-opus-gateway",
      type: "set_default_model",
    });
    child.emitStdout(
      `${JSON.stringify({
        id: setDefault.id,
        payload: { modelId: "claude-opus-gateway", envOverride: false, warnings: [] },
        success: true,
        type: "response",
      })}\n`,
    );
    await expect(pendingDefault).resolves.toMatchObject({ success: true, payload: { envOverride: false } });

    const pendingRemove = messenger.sendRemoveModel("claude-opus-gateway");
    const remove = readSingleCommandLine(child);
    expect(remove).toMatchObject({
      modelId: "claude-opus-gateway",
      type: "remove_model",
    });
    child.emitStdout(
      `${JSON.stringify({
        id: remove.id,
        payload: null,
        success: true,
        type: "response",
      })}\n`,
    );
    await expect(pendingRemove).resolves.toMatchObject({ success: true });
  });
});
