import type { ModelPickerModel } from "./ModelPicker";

function compact(value: string): string {
  return value.toLocaleLowerCase().replace(/[-._/\s]+/g, "");
}

function queryTokens(query: string): string[] {
  return query
    .trim()
    .split(/\s+/)
    .map(compact)
    .filter(Boolean);
}

/**
 * Matches every normalized query token against a model id or upstream model
 * name. Descriptions are intentionally excluded: prose should not make an
 * unrelated model appear in a picker search.
 */
export function filterModels<T extends Pick<ModelPickerModel, "id" | "modelName">>(
  models: readonly T[],
  query: string,
): readonly T[] {
  const tokens = queryTokens(query);
  if (tokens.length === 0) {
    return models;
  }

  return models.filter((model) => {
    const candidates = [compact(model.id), compact(model.modelName ?? "")];
    return tokens.every((token) => candidates.some((candidate) => candidate.includes(token)));
  });
}
