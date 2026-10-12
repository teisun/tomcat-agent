import { describe, expect, it } from "vitest";

import { formatModelLabel, thinkingLevelLabel } from "./modelLabel";

describe("formatModelLabel", () => {
  it("combines a model id with the selected reasoning tier", () => {
    expect(
      formatModelLabel({
        modelId: "gpt-5.6",
        selectedReasoningLevel: "xhigh",
        supportedReasoningLevels: ["low", "high", "xhigh"],
      }),
    ).toBe("gpt-5.6 Xhigh");
  });

  it("degrades to the model id when reasoning is unsupported or missing", () => {
    expect(
      formatModelLabel({
        modelId: "plain-model",
        selectedReasoningLevel: "high",
        supportedReasoningLevels: [],
      }),
    ).toBe("plain-model");
    expect(
      formatModelLabel({
        modelId: "gpt-5.6",
        supportedReasoningLevels: ["low", "high"],
      }),
    ).toBe("gpt-5.6");
  });

  it("renders a resolved single effort tier", () => {
    expect(
      formatModelLabel({
        modelId: "mimo-v2.5-pro",
        selectedReasoningLevel: "high",
        supportedReasoningLevels: ["high"],
      }),
    ).toBe("mimo-v2.5-pro High");
  });

  it("keeps special tier names consistent with existing composer labels", () => {
    expect(thinkingLevelLabel("off")).toBe("Off");
    expect(thinkingLevelLabel("max")).toBe("Max");
    expect(thinkingLevelLabel("xhigh")).toBe("Xhigh");
  });
});
