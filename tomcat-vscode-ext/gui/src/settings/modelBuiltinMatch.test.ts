import { describe, expect, it } from "vitest";

import type { SettingsModelView } from "../../../src/shared/settingsProtocol";
import { findReusableModelByName } from "./modelBuiltinMatch";

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
