import { describe, expect, it } from "vitest";

import { parseInitializePayload } from "../protocol";

describe("serve client protocol helpers", () => {
  it("parses serverVersion when the initialize payload includes it", () => {
    expect(
      parseInitializePayload({
        capabilities: ["prompt", "ask_question"],
        protocolVersion: 1,
        serverVersion: "0.1.17",
        sessionId: "s1",
      }),
    ).toEqual({
      capabilities: ["prompt", "ask_question"],
      protocolVersion: 1,
      serverVersion: "0.1.17",
      sessionId: "s1",
    });
  });

  it("treats missing or invalid serverVersion as null", () => {
    expect(
      parseInitializePayload({
        capabilities: ["prompt", "ask_question"],
        protocolVersion: 1,
      }).serverVersion,
    ).toBeNull();

    expect(
      parseInitializePayload({
        capabilities: ["prompt", "ask_question"],
        protocolVersion: 1,
        serverVersion: 114,
      }).serverVersion,
    ).toBeNull();
  });
});
