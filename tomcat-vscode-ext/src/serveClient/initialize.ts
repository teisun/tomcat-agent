import { randomUUID } from "node:crypto";

import { assertRequiredCapabilities, parseInitializePayload } from "./protocol";
import type { TomcatMessenger } from "./TomcatMessenger";

export interface InitializeResult {
  capabilities: string[];
  protocolVersion: number;
  sessionId: string | null;
}

export async function initializeServe(
  messenger: TomcatMessenger,
): Promise<InitializeResult> {
  const frame = await messenger.requestControl({
    payload: null,
    requestId: `init-${randomUUID()}`,
    subtype: "initialize",
    type: "control_request",
  });

  if (frame.type !== "control_response") {
    throw new Error(`initialize was cancelled for request ${frame.requestId}`);
  }

  const payload = parseInitializePayload(frame.payload);
  assertRequiredCapabilities(payload.capabilities);

  return {
    capabilities: payload.capabilities,
    protocolVersion: payload.protocolVersion,
    sessionId: payload.sessionId ?? frame.sessionId ?? null,
  };
}
