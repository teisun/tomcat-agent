import { describe, expect, it } from "vitest";

import { deriveRelayFields, envNameForRelaySlug } from "./relayDerive";

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

  it("builds env names directly from relay slugs", () => {
    expect(envNameForRelaySlug("chatanywhere")).toBe("CHATANYWHERE_API_KEY");
    expect(envNameForRelaySlug("local_llm")).toBe("LOCAL_LLM_API_KEY");
  });
});
