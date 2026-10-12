import { describe, expect, it } from "vitest";

import { parseInitializePayload } from "../protocol";

describe("shared slash handshake", () => {
  it("uses an empty table for a legacy server and preserves the backend's table", () => {
    const payload = { capabilities:["prompt"], protocolVersion:2 };
    expect(parseInitializePayload(payload).slashCommands).toEqual([]);
    const slashCommands = [{name:"reload", usage:"/reload", summary:"重扫"}];
    expect(parseInitializePayload({...payload, slashCommands}).slashCommands).toEqual(slashCommands);
  });
  it.each([null, "bad", [{usage:"/reload", summary:"x"}], [{name:"reload", usage:42, summary:"x"}], [{name:"reload", usage:"/reload"}]])("rejects malformed command entries: %j", (slashCommands) => {
    expect(() => parseInitializePayload({capabilities:[], protocolVersion:2, slashCommands})).toThrow("slashCommands");
  });
});

describe("serve client protocol helpers", () => {
  it("parses serverVersion when the initialize payload includes it", () => {
    expect(
      parseInitializePayload({
        capabilities: ["prompt", "ask_question"],
        protocolVersion: 1,
        serverVersion: "0.1.20",
        sessionId: "s1",
      }),
    ).toEqual({
      attachmentRoot: null,
      slashCommands: [],
      capabilities: ["prompt", "ask_question"],
      protocolVersion: 1,
      serverVersion: "0.1.20",
      sessionId: "s1",
    });
  });

  it("parses the attachment root, which the host needs before it renders the webview", () => {
    expect(
      parseInitializePayload({
        attachmentRoot: "/home/u/.tomcat/sessions/attachments",
        capabilities: ["prompt", "ask_question"],
        protocolVersion: 1,
      }).attachmentRoot,
    ).toBe("/home/u/.tomcat/sessions/attachments");

    // An older server simply omits it; images then have nowhere to load from, which the
    // host degrades to "unavailable" rather than guessing at a path.
    expect(
      parseInitializePayload({
        capabilities: ["prompt", "ask_question"],
        protocolVersion: 1,
      }).attachmentRoot,
    ).toBeNull();
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
