import { describe, expect, it } from "vitest";

import type { SettingsModelView } from "../../../src/shared/settingsProtocol";
import { defaultProtocolFor, defaultSupportsFast, findReusableModelByName } from "./modelBuiltinMatch";

function builtinModel(overrides: Partial<SettingsModelView> = {}): SettingsModelView {
  return {
    api: "openai",
    apiKeyEnv: "TEST_API_KEY",
    capabilities: {
      files: false,
      reasoning: true,
      tools: true,
      vision: false,
      webSearch: false,
    },
    id: "deepseek-v4-flash",
    keyPresent: false,
    modelName: "deepseek-v4-flash",
    provider: "deepseek",
    source: "builtin",
    ...overrides,
  };
}

describe("findReusableModelByName", () => {
  it("matches a relay-facing model name instead of its Tomcat id", () => {
    const match = findReusableModelByName(
      [builtinModel({ id: "gpt-5.6", modelName: "gpt-5.6" })],
      "gpt-5.6",
    );

    expect(match?.id).toBe("gpt-5.6");
  });

  it("uses the first built-in entry in catalog order when names collide", () => {
    const first = builtinModel({
      contextWindow: 128000,
      id: "deepseek-v4-flash",
    });
    const second = builtinModel({
      contextWindow: 256000,
      id: "utility-flash",
    });

    expect(findReusableModelByName([first, second], "deepseek-v4-flash")).toBe(first);
  });

  it("prefers the user's configured model over a built-in model with the same upstream name", () => {
    const user = builtinModel({
      id: "relay/deepseek-v4-flash",
      provider: "existing-relay",
      source: "user",
    });
    const builtIn = builtinModel();

    expect(findReusableModelByName([builtIn, user], "deepseek-v4-flash")).toBe(user);
  });

  it("returns null for blank or unknown names", () => {
    expect(findReusableModelByName([], "  ")).toBeNull();
    expect(findReusableModelByName([], "unknown")).toBeNull();
  });
});

const ALL_PROTOCOLS = ["openai", "openai-responses", "anthropic-messages"];

describe("defaultProtocolFor", () => {
  it("follows the reference model, then claude- names, then the connection's primary protocol", () => {
    expect(defaultProtocolFor("gpt-6.1-sol", { api: "openai-responses" }, ALL_PROTOCOLS)).toBe("openai-responses");
    expect(defaultProtocolFor("claude-opus-5-5", { api: "anthropic-messages" }, ALL_PROTOCOLS)).toBe("anthropic-messages");
    expect(defaultProtocolFor("anthropic/claude-sonnet-5", null, ALL_PROTOCOLS)).toBe("anthropic-messages");
    expect(defaultProtocolFor("Claude-X", null, ALL_PROTOCOLS)).toBe("anthropic-messages");
    expect(defaultProtocolFor("my-relay-model", null, ALL_PROTOCOLS)).toBe("openai");
    expect(defaultProtocolFor("gpt-7-preview", null, ALL_PROTOCOLS)).toBe("openai");
  });

  it("never leaves the candidates of a single-protocol preset", () => {
    expect(defaultProtocolFor("claude-x", null, ["openai"])).toBe("openai");
    expect(defaultProtocolFor("gpt-6.1-sol", { api: "openai-responses" }, ["openai"])).toBe("openai");
  });
});

describe("defaultSupportsFast", () => {
  it("declares Fast for GPT and Claude names by their last path segment, case-insensitively", () => {
    expect(defaultSupportsFast("gpt-6.1-sol", null)).toBe(true);
    expect(defaultSupportsFast("GPT-6.1-Sol", null)).toBe(true);
    expect(defaultSupportsFast("Claude-Opus-5", null)).toBe(true);
    expect(defaultSupportsFast("openai/gpt-x", null)).toBe(true);
    expect(defaultSupportsFast("anthropic/claude-sonnet-5", null)).toBe(true);
  });

  it("leaves other names undeclared unless the reference model declares fast", () => {
    for (const name of ["o3", "codex-mini", "deepseek-chat", "kimi-k3"]) {
      expect(defaultSupportsFast(name, null)).toBe(false);
    }
    expect(defaultSupportsFast("my-model", { supportedSpeeds: ["fast"] })).toBe(true);
    expect(defaultSupportsFast("my-model", { supportedSpeeds: [] })).toBe(false);
  });

  it("uses the user's same-name entry as reference, while GPT names still get Fast", () => {
    const user = builtinModel({ id: "relay/gpt-5.6", modelName: "gpt-5.6", source: "user", supportedSpeeds: [] });
    const builtIn = builtinModel({ id: "gpt-5.6", modelName: "gpt-5.6", supportedSpeeds: ["fast"] });
    const reference = findReusableModelByName([builtIn, user], "gpt-5.6");
    expect(reference).toBe(user);
    expect(defaultSupportsFast("gpt-5.6", reference)).toBe(true);
  });
});
