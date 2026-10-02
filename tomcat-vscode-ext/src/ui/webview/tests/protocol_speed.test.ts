import { describe, expect, it } from "vitest";
import { isWebviewIntent } from "../protocol";
import { isPlanPreviewIntent } from "../../../shared/planPreviewProtocol";

for (const [name, validate] of [["chat", isWebviewIntent], ["plan", isPlanPreviewIntent]] as const) {
  describe(`${name} setSpeed intent`, () => {
    it.each(["standard", "fast", "ultrafast"])("accepts %s", (speed) => {
      expect(validate({ messageId: "speed", type: "setSpeed", data: { modelId: "relay/model", speed } })).toBe(true);
    });
    it.each(["warp", "priority", "", 1, null, undefined])("rejects invalid %s", (speed) => {
      expect(validate({ messageId: "speed", type: "setSpeed", data: { modelId: "relay/model", speed } })).toBe(false);
    });
    it("rejects the legacy field name", () => {
      expect(validate({ messageId: "speed", type: "setSpeed", data: { modelId: "relay/model", tier: "fast" } })).toBe(false);
    });
    it("requires a model ID", () => {
      expect(validate({ messageId: "speed", type: "setSpeed", data: { speed: "fast" } })).toBe(false);
    });
  });
}
