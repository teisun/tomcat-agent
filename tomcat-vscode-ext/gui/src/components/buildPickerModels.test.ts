import { describe, expect, it } from "vitest";

import { buildPickerModels } from "./buildPickerModels";

describe("buildPickerModels", () => {
  it("keeps a selected model visible when it is absent from the catalog", () => {
    expect(
      buildPickerModels({
        availableModels: ["gpt-5.6"],
        selectedModelId: "stale-build-model",
      }).map((model) => model.id),
    ).toEqual(["stale-build-model", "gpt-5.6"]);
  });

  it("overlays active session context and effort without changing other catalog rows", () => {
    const models = buildPickerModels({
      activeModelId: "gpt-5.6",
      availableModelDetails: {
        "claude-opus": {
          contextWindowOptions: [200_000, 400_000],
          id: "claude-opus",
          selectedContextWindow: 200_000,
          selectedReasoningLevel: "high",
          supportedReasoningLevels: ["low", "high"],
        },
        "gpt-5.6": {
          contextWindowOptions: [400_000, 1_000_000],
          id: "gpt-5.6",
          selectedContextWindow: 400_000,
          selectedReasoningLevel: "high",
          supportedReasoningLevels: ["low", "high", "xhigh"],
        },
      },
      availableModels: ["gpt-5.6", "claude-opus"],
      sessionContextWindow: 1_000_000,
      sessionThinkingLevel: "xhigh",
    });

    expect(models).toEqual([
      expect.objectContaining({
        id: "gpt-5.6",
        selectedContextWindow: 1_000_000,
        selectedReasoningLevel: "xhigh",
      }),
      expect.objectContaining({
        id: "claude-opus",
        selectedContextWindow: 200_000,
        selectedReasoningLevel: "high",
      }),
    ]);
  });

  it("fills missing reasoning tiers from the legacy catalog field", () => {
    expect(
      buildPickerModels({
        availableModelReasoningLevels: {
          "gpt-5.6": ["low", "high"],
        },
        availableModels: ["gpt-5.6"],
      }),
    ).toEqual([
      expect.objectContaining({
        id: "gpt-5.6",
        supportedReasoningLevels: ["low", "high"],
      }),
    ]);
  });
});
