import { describe, expect, it } from "vitest";

import { planEventState } from "../planState";

describe("planEventState", () => {
  it("maps plan-file lifecycle events but not legacy mode events", () => {
    expect(planEventState({ type: "plan.pending" })).toBe("pending");
    expect(planEventState({ type: "plan.stalled" })).toBe("pending");
    expect(planEventState({ type: "plan.enter" })).toBeNull();
    expect(planEventState({ type: "plan.exit" })).toBeNull();
  });

  it("keeps plan.complete mapped to completed", () => {
    expect(planEventState({ type: "plan.complete" })).toBe("completed");
  });

  it("prefers explicit state over derived event defaults", () => {
    expect(
      planEventState({
        state: "pending",
        type: "plan.build",
      }),
    ).toBe("pending");
    expect(
      planEventState({
        state: "executing",
        type: "plan.exit",
      }),
    ).toBe("executing");
  });
});
