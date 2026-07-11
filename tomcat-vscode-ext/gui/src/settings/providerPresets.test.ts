import { describe, expect, it } from "vitest";

import type { SettingsModelView } from "../../../src/shared/settingsProtocol";
import {
  buildProviderPresets,
  findMatchingProviderPreset,
} from "./providerPresets";

function builtinModel(overrides: Partial<SettingsModelView> = {}): SettingsModelView {
  return {
    api: "openai-responses",
    apiKeyEnv: "OPENAI_API_KEY",
    baseUrl: "https://api.openai.com",
    capabilities: {
      files: true,
      reasoning: true,
      tools: true,
      vision: true,
      webSearch: false,
    },
    contextWindow: 400000,
    id: "gpt-5.4",
    keyPresent: false,
    modelName: "gpt-5.4",
    provider: "openai",
    source: "builtin",
    thinkingFormat: "openai",
    ...overrides,
  };
}

describe("providerPresets", () => {
  it("groups builtin models by provider and keeps authoritative preset fields", () => {
    const presets = buildProviderPresets([
      builtinModel({
        id: "gpt-5.4",
        keyPresent: true,
      }),
      builtinModel({
        api: "openai",
        apiKeyEnv: "DEEPSEEK_API_KEY",
        baseUrl: "https://api.deepseek.com",
        capabilities: {
          files: false,
          reasoning: true,
          tools: true,
          vision: false,
          webSearch: false,
        },
        id: "deepseek-v4-pro",
        keyPresent: false,
        modelName: "deepseek-v4-pro",
        provider: "deepseek",
        thinkingFormat: "deepseek",
      }),
      builtinModel({
        id: "gpt-5.6",
        keyPresent: false,
        modelName: "gpt-5.6",
      }),
      {
        ...builtinModel(),
        api: "openai",
        apiKeyEnv: "CHATANYWHERE_API_KEY",
        baseUrl: "https://api.chatanywhere.tech/v1",
        id: "chatanywhere/gpt-5.4",
        keyPresent: false,
        modelName: "gpt-5.4",
        provider: "chatanywhere",
        source: "user",
      },
    ]);

    expect(presets).toHaveLength(2);
    expect(presets[0]).toMatchObject({
      api: "openai-responses",
      apiKeyEnv: "OPENAI_API_KEY",
      baseUrl: "https://api.openai.com",
      keyPresent: true,
      modelIds: ["gpt-5.4", "gpt-5.6"],
      provider: "openai",
      thinkingFormat: "openai",
    });
    expect(presets[1]).toMatchObject({
      api: "openai",
      apiKeyEnv: "DEEPSEEK_API_KEY",
      baseUrl: "https://api.deepseek.com",
      keyPresent: false,
      modelIds: ["deepseek-v4-pro"],
      provider: "deepseek",
      thinkingFormat: "deepseek",
    });
  });

  it("returns an empty list for empty input and matches presets by provider plus route", () => {
    expect(buildProviderPresets([])).toEqual([]);
    expect(
      buildProviderPresets([
        {
          ...builtinModel(),
          id: "openai/gpt-5.4",
          source: "user",
        },
      ]),
    ).toEqual([]);

    const presets = buildProviderPresets([builtinModel()]);
    expect(
      findMatchingProviderPreset(
        {
          api: "openai-responses",
          baseUrl: "https://api.openai.com",
          provider: "openai",
        },
        presets,
      )?.provider,
    ).toBe("openai");
    expect(
      findMatchingProviderPreset(
        {
          api: "openai-responses",
          baseUrl: "https://gateway.example.test/v1",
          provider: "openai",
        },
        presets,
      )?.provider,
    ).toBe("openai");
    expect(
      findMatchingProviderPreset(
        {
          api: "openai",
          baseUrl: "https://api.chatanywhere.tech/v1",
          provider: "openai",
        },
        presets,
      ),
    ).toBeNull();
  });
});
