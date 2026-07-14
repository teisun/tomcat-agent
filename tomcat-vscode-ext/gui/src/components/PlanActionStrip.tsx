import { PlanBuildModelSelect } from "./PlanBuildModelSelect";

/**
 * Hybrid (B) in-body action strip: the model dropdown plus the yellow Build
 * button, rendered once at the top of the plan content. It carries no file
 * path and no Preview/Markdown toggle (both live on the native title bar) and
 * it does not stick — VS Code's own editor title bar already floats.
 */
export function PlanActionStrip({
  availableModels,
  buildModel,
  canBuild,
  onBuild,
  onSetBuildModel,
}: {
  availableModels: string[];
  buildModel: string;
  canBuild: boolean;
  onBuild(): void;
  onSetBuildModel(modelId: string): void;
}) {
  return (
    <div className="tc-plan-action-strip" data-testid="plan-action-strip">
      <PlanBuildModelSelect
        availableModels={availableModels}
        onChange={onSetBuildModel}
        value={buildModel}
      />
      <button
        className="tc-button tc-plan-build-button"
        data-testid="plan-build"
        disabled={!canBuild}
        onClick={onBuild}
        type="button"
      >
        Build
      </button>
    </div>
  );
}
