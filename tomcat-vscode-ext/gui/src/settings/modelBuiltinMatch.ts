import type { SettingsModelView } from "../../../src/shared/settingsProtocol";

/**
 * Finds the best reusable specification for a provider-facing model name.
 *
 * This intentionally differs from the write-collision check: a relay model
 * may use a different Tomcat id but still refer to the same upstream model.
 * A user models.toml entry is an explicit local decision, so it wins over the
 * embedded catalog. Catalog order gives each group a stable, explainable winner.
 */
export function findReusableModelByName(
  models: readonly SettingsModelView[],
  modelName: string | null | undefined,
): SettingsModelView | null {
  const normalizedName = modelName?.trim();
  if (!normalizedName) return null;
  const matchesName = (model: SettingsModelView) =>
    (model.modelName?.trim() || model.id.trim()) === normalizedName;
  return (
    models.find((model) => model.source === "user" && matchesName(model)) ??
    models.find((model) => model.source === "builtin" && matchesName(model)) ??
    null
  );
}

/** Aggregating relays prefix names (`anthropic/claude-…`); only the last segment names the family. */
function upstreamFamilyName(modelName: string): string {
  const lowered = modelName.trim().toLowerCase();
  return lowered.slice(lowered.lastIndexOf("/") + 1);
}

/**
 * Default protocol for a new model; always one of `candidates`, whose first
 * entry is the connection's primary protocol.
 */
export function defaultProtocolFor(
  modelName: string,
  reference: Pick<SettingsModelView, "api"> | null,
  candidates: readonly string[],
): string {
  if (reference && candidates.includes(reference.api)) return reference.api;
  if (upstreamFamilyName(modelName).startsWith("claude-") && candidates.includes("anthropic-messages")) {
    return "anthropic-messages";
  }
  return candidates[0] ?? "openai";
}

/** Computed once for a new draft only; existing entries are never re-defaulted. */
export function defaultSupportsFast(
  modelName: string,
  reference: Pick<SettingsModelView, "supportedSpeeds"> | null,
): boolean {
  const family = upstreamFamilyName(modelName);
  return family.startsWith("gpt-") || family.startsWith("claude-") || reference?.supportedSpeeds?.includes("fast") === true;
}
