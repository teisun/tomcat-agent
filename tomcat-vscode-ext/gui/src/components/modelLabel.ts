import { t as defaultT, type Translator } from "../../../src/shared/i18n";

export function thinkingLevelLabel(value: string | null | undefined, t: Translator = defaultT): string {
  const normalized = value?.trim().toLowerCase() ?? "";
  switch (normalized) {
    case "":
      return "";
    case "off":
    case "minimal":
    case "low":
    case "medium":
    case "high":
    case "xhigh":
    case "max":
      return t(`term.effort.${normalized}`);
    default:
      return titleCaseToken(normalized);
  }
}

/**
 * Renders the single compact label used by all model-picker triggers.
 *
 * A reasoning suffix is deliberately shown only when the model declares one
 * or more selectable reasoning tiers and a selected tier is available. This
 * keeps plain models and incomplete state snapshots from pretending to have a
 * reasoning setting.
 */
export function formatModelLabel({
  modelId,
  selectedReasoningLevel,
  supportedReasoningLevels,
}: {
  modelId: string | null | undefined;
  selectedReasoningLevel?: string | null;
  supportedReasoningLevels?: readonly string[] | null;
}, t: Translator = defaultT): string {
  const { id, reasoning } = modelLabelParts({
    modelId,
    selectedReasoningLevel,
    supportedReasoningLevels,
  }, t);
  return reasoning ? `${id} ${reasoning}` : id;
}

export function modelLabelParts({
  modelId,
  selectedReasoningLevel,
  supportedReasoningLevels,
}: {
  modelId: string | null | undefined;
  selectedReasoningLevel?: string | null;
  supportedReasoningLevels?: readonly string[] | null;
}, t: Translator = defaultT): { id: string; reasoning: string } {
  const id = modelId?.trim() ?? "";
  if (!id) {
    return { id: t("model.label"), reasoning: "" };
  }
  if (!supportedReasoningLevels?.some((level) => level.trim())) {
    return { id, reasoning: "" };
  }
  const reasoning = thinkingLevelLabel(selectedReasoningLevel, t);
  return { id, reasoning };
}

function titleCaseToken(value: string): string {
  return value ? value[0].toUpperCase() + value.slice(1) : "";
}
