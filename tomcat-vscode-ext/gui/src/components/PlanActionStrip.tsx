import { useT } from "../i18n/LocaleProvider";
import type { Speed } from "../../../src/shared/modelSpeed";
import { buildPickerModels } from "./buildPickerModels";
import { ModelPicker, type ModelPickerModel } from "./ModelPicker";
import type { PlanFileState } from "../../../src/shared/planPreviewProtocol";

/**
 * Hybrid (B) in-body action strip: the model dropdown plus the yellow Build
 * button, rendered once at the top of the plan content. It carries no file
 * path and no Preview/Markdown toggle (both live on the native title bar) and
 * it does not stick — VS Code's own editor title bar already floats.
 */
export function PlanActionStrip({
  availableModelDetails,
  availableModels,
  buildModel,
  canBuild,
  fileState,
  onBuild,
  onSelectContextWindow,
  onSelectThinkingLevel,
  onSelectSpeed,
  onSetBuildModel,
  sessionContextWindow,
  sessionModel,
  sessionThinkingLevel,
}: {
  availableModelDetails?: Record<string, ModelPickerModel>;
  availableModels: string[];
  buildModel: string;
  canBuild: boolean;
  fileState: PlanFileState | null;
  onBuild(): void;
  onSelectContextWindow(modelId: string, contextWindow: number): void;
  onSelectThinkingLevel(modelId: string, level: string): void;
  onSelectSpeed(modelId: string, speed: Speed): void;
  onSetBuildModel(modelId: string): void;
  sessionContextWindow?: number | null;
  sessionModel: string;
  sessionThinkingLevel?: string | null;
}) {
  const selectedModelId = buildModel || sessionModel || null;
  const t = useT();
  const pickerModels = buildPickerModels({
    activeModelId: sessionModel,
    availableModelDetails,
    availableModels,
    selectedModelId,
    sessionContextWindow,
    sessionThinkingLevel,
  });

  return (
    <div className="tc-plan-action-strip" data-testid="plan-action-strip">
      <ModelPicker
        className="tc-plan-model-picker"
        disabled={pickerModels.length === 0}
        label={t("plan.buildModel")}
        models={pickerModels}
        onSelectContextWindow={onSelectContextWindow}
        onSelectModel={onSetBuildModel}
        onSelectThinkingLevel={onSelectThinkingLevel}
        onSelectSpeed={onSelectSpeed}
        placement="below"
        selectedModelId={selectedModelId}
        testId="plan-build-model-select"
      />
      <button
        className="tc-button tc-plan-build-button"
        data-testid="plan-build"
        disabled={!canBuild}
        onClick={onBuild}
        type="button"
      >
        {t(fileState === "pending" ? "common.resume" : "plan.build")}
      </button>
    </div>
  );
}
