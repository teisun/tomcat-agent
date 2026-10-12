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

/** @deprecated Use findReusableModelByName. */
export const findBuiltinModelByName = findReusableModelByName;
