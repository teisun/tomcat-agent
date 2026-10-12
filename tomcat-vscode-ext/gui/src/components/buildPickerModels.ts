import type { ModelPickerModel } from "./ModelPicker";

export interface BuildPickerModelsOptions {
  activeModelId?: string | null;
  availableModelDetails?: Readonly<Record<string, ModelPickerModel>>;
  availableModelReasoningLevels?: Readonly<Record<string, readonly string[]>>;
  availableModels: readonly string[];
  selectedModelId?: string | null;
  sessionContextWindow?: number | null;
  sessionThinkingLevel?: string | null;
}

/**
 * Combines catalog metadata with the active session's selections without
 * mutating the catalog snapshot shared by the composer and plan surfaces.
 */
export function buildPickerModels({
  activeModelId,
  availableModelDetails,
  availableModelReasoningLevels,
  availableModels,
  selectedModelId,
  sessionContextWindow,
  sessionThinkingLevel,
}: BuildPickerModelsOptions): ModelPickerModel[] {
  const modelIds =
    selectedModelId && !availableModels.includes(selectedModelId)
      ? [selectedModelId, ...availableModels]
      : [...availableModels];

  return modelIds.map((id) => {
    const details = availableModelDetails?.[id];
    const isActiveModel = id === activeModelId;
    return {
      ...details,
      id,
      selectedContextWindow:
        isActiveModel && sessionContextWindow !== undefined
          ? sessionContextWindow
          : details?.selectedContextWindow,
      selectedReasoningLevel:
        isActiveModel && sessionThinkingLevel?.trim()
          ? sessionThinkingLevel
          : details?.selectedReasoningLevel,
      supportedReasoningLevels:
        details?.supportedReasoningLevels ?? availableModelReasoningLevels?.[id],
    };
  });
}
