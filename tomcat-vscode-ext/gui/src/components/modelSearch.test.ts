import { describe, expect, it } from "vitest";

import { filterModels } from "./modelSearch";

const MODELS = [
  {
    id: "fcodex/gpt-5.6-terra",
    modelName: "gpt-5.6-terra",
  },
  {
    id: "openai/gpt-5.6",
    modelName: "gpt-5.6",
  },
  {
    id: "anthropic/claude-opus-4.8",
    modelName: "claude-opus-4-8",
  },
  {
    id: "plain-model",
    modelName: "fast general model",
  },
] as const;

function ids(query: string): string[] {
  return filterModels(MODELS, query).map((model) => model.id);
}

describe("filterModels", () => {
  it.each([
    ["56", ["fcodex/gpt-5.6-terra", "openai/gpt-5.6"]],
    ["gpt56", ["fcodex/gpt-5.6-terra", "openai/gpt-5.6"]],
    ["opus48", ["anthropic/claude-opus-4.8"]],
    ["5.6 terra", ["fcodex/gpt-5.6-terra"]],
    ["terra 5.6", ["fcodex/gpt-5.6-terra"]],
  ])("matches %s across normalized model identifiers", (query, expected) => {
    expect(ids(query)).toEqual(expected);
  });

  it("does not match model descriptions or partial tokens that are absent", () => {
    expect(ids("reasoning")).toEqual([]);
    expect(ids("gpt 4")).toEqual([]);
    expect(ids("unknown")).toEqual([]);
  });
});
