import { describe, expect, it } from "vitest";

import { PLAN_PREVIEW_EDITOR_OPTIONS } from "./registrationOptions";

describe("Plan preview editor registration options", () => {
  it("disables the native Find Widget because Plan Preview owns the counted search UI", () => {
    expect(PLAN_PREVIEW_EDITOR_OPTIONS).toEqual({
      supportsMultipleEditorsPerDocument: false,
      webviewOptions: {
        enableFindWidget: false,
        retainContextWhenHidden: true,
      },
    });
  });
});
