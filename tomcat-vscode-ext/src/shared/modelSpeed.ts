import type { Speed } from "../serveClient/wire";

export type { Speed };
export const SPEEDS: readonly Speed[] = ["standard", "fast"];

export function isSpeed(value: unknown): value is Speed {
  return typeof value === "string" && SPEEDS.includes(value as Speed);
}
