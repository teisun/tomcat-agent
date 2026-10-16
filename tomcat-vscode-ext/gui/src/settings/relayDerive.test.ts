import { describe, expect, it } from "vitest";

import type { SettingsModelView } from "../../../src/shared/settingsProtocol";
import {
  deriveRelayFields,
  envNameForRelaySlug,
  findConfiguredRelayByBaseUrl,
} from "./relayDerive";

function relayModel(overrides: Partial<SettingsModelView>): SettingsModelView {
  return {
    api: "openai",
    apiKeyEnv: "FCODEX_API_KEY",
    baseUrl: "https://fcodex.example.com/v1",
    capabilities: { files: false, reasoning: true, tools: true, vision: false, webSearch: false },
    id: "fcodex/gpt-6.1-sol",
    keyPresent: true,
    modelName: "gpt-6.1-sol",
    provider: "fcodex",
    source: "user",
    ...overrides,
  };
}

describe("relayDerive", () => {
  it("derives stable slugs from common relay hosts", () => {
    expect(deriveRelayFields("api.chatanywhere.tech/v1", "gpt-5.4")).toMatchObject({
      apiKeyEnv: "CHATANYWHERE_API_KEY",
      host: "api.chatanywhere.tech",
      id: "chatanywhere/gpt-5.4",
      provider: "chatanywhere",
      slug: "chatanywhere",
    });

    expect(
      deriveRelayFields("https://open.bigmodel.cn/api/paas/v4", "glm-5.2"),
    ).toMatchObject({
      apiKeyEnv: "BIGMODEL_API_KEY",
      id: "bigmodel/glm-5.2",
      provider: "bigmodel",
    });

    expect(deriveRelayFields("https://foo.com.cn/v1", "kimi-k2")).toMatchObject({
      apiKeyEnv: "FOO_API_KEY",
      id: "foo/kimi-k2",
      provider: "foo",
    });
  });

  it("handles ports, IPs, localhost, IDNs, and parse failures without throwing", () => {
    expect(deriveRelayFields("http://localhost:11434/v1", "qwen3")).toMatchObject({
      apiKeyEnv: "LOCALHOST_API_KEY",
      id: "localhost/qwen3",
      provider: "localhost",
    });

    expect(deriveRelayFields("http://127.0.0.1:4000/v1", "gpt-oss")).toMatchObject({
      apiKeyEnv: "127001_API_KEY",
      id: "127001/gpt-oss",
      provider: "127001",
    });

    const idn = deriveRelayFields("https://例子.测试/v1", "glm");
    expect(idn.provider.length).toBeGreaterThan(0);
    expect(idn.apiKeyEnv.endsWith("_API_KEY")).toBe(true);
    expect(idn.id.endsWith("/glm")).toBe(true);

    expect(deriveRelayFields("::::", "gpt-5.4")).toMatchObject({
      apiKeyEnv: "CUSTOM_API_KEY",
      id: "custom/gpt-5.4",
      provider: "custom",
    });

    expect(deriveRelayFields("", "gpt-5.4")).toMatchObject({
      apiKeyEnv: "",
      id: "",
      provider: "",
      slug: "",
    });
  });

  it("names one slot per key, independent of protocol, and numbers taken names", () => {
    expect(envNameForRelaySlug("chatanywhere")).toBe("CHATANYWHERE_API_KEY");
    expect(envNameForRelaySlug("local_llm")).toBe("LOCAL_LLM_API_KEY");
    expect(envNameForRelaySlug("fcodex", ["FCODEX_API_KEY"])).toBe("FCODEX_2_API_KEY");
    expect(envNameForRelaySlug("fcodex", ["FCODEX_API_KEY", "FCODEX_2_API_KEY"])).toBe("FCODEX_3_API_KEY");
    expect(envNameForRelaySlug("openai", ["OPENAI_API_KEY"])).toBe("OPENAI_2_API_KEY");
    // Legacy per-protocol slots are left alone and do not block the new name.
    expect(envNameForRelaySlug("fcodex", ["FCODEX_OPENAI_API_KEY", "FCODEX_ANTHROPIC_API_KEY"])).toBe("FCODEX_API_KEY");
    expect(deriveRelayFields("https://api.openai.com/v1", "gpt-x", ["OPENAI_API_KEY"]).apiKeyEnv).toBe("OPENAI_2_API_KEY");
  });

  it("returns every key slot at the same endpoint instead of silently inheriting the first", () => {
    const models = [
      relayModel({ apiKeyEnv: "FCODEX_OPENAI_API_KEY" }),
      relayModel({ api: "anthropic-messages", apiKeyEnv: "FCODEX_ANTHROPIC_API_KEY", baseUrl: "https://FCODEX.example.com/v1/", id: "fcodex/claude-opus-5-5" }),
      relayModel({ apiKeyEnv: "FCODEX_OPENAI_API_KEY", id: "fcodex/gpt-5.5" }),
      relayModel({ apiKeyEnv: "OTHER_API_KEY", baseUrl: "https://other.example.com/v1", id: "other/gpt" }),
      relayModel({ apiKeyEnv: "DEEPSEEK_API_KEY", baseUrl: "https://api.deepseek.com", id: "deepseek-v4-pro", source: "builtin" }),
    ];

    expect(findConfiguredRelayByBaseUrl(models, "https://fcodex.example.com/v1")).toEqual([
      "FCODEX_OPENAI_API_KEY",
      "FCODEX_ANTHROPIC_API_KEY",
    ]);
    expect(findConfiguredRelayByBaseUrl(models, "https://api.deepseek.com/")).toEqual(["DEEPSEEK_API_KEY"]);
    expect(findConfiguredRelayByBaseUrl(models, "https://nowhere.example.com")).toEqual([]);
    expect(findConfiguredRelayByBaseUrl(models, "")).toEqual([]);
  });
});
