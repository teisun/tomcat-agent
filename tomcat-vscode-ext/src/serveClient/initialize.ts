import { randomUUID } from "node:crypto";

import { assertRequiredCapabilities, parseInitializePayload } from "./protocol";
import type { TomcatMessenger } from "./TomcatMessenger";

export const SERVE_CAPABILITY_LIST_MODELS = "list_models";
export const SERVE_CAPABILITY_LIST_PROVIDER_KEYS = "list_provider_keys";
export const SERVE_CAPABILITY_SET_PLAN_MODE = "set_plan_mode";
export const SERVE_CAPABILITY_SET_PROVIDER_KEY = "set_provider_key";
export const SERVE_CAPABILITY_UPSERT_MODEL = "upsert_model";
export const SERVE_CAPABILITY_REMOVE_MODEL = "remove_model";
export const SERVE_MODEL_ADMIN_CAPABILITIES = [
  SERVE_CAPABILITY_LIST_MODELS,
  SERVE_CAPABILITY_LIST_PROVIDER_KEYS,
  SERVE_CAPABILITY_REMOVE_MODEL,
  SERVE_CAPABILITY_SET_PROVIDER_KEY,
  SERVE_CAPABILITY_UPSERT_MODEL,
] as const;

export interface InitializeResult {
  capabilities: string[];
  protocolVersion: number;
  sessionId: string | null;
}

export function hasServeCapability(
  result: Pick<InitializeResult, "capabilities">,
  capability: string,
): boolean {
  return result.capabilities.includes(capability);
}

export function hasModelAdminCapabilities(
  result: Pick<InitializeResult, "capabilities">,
): boolean {
  return SERVE_MODEL_ADMIN_CAPABILITIES.every((capability) =>
    hasServeCapability(result, capability),
  );
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
