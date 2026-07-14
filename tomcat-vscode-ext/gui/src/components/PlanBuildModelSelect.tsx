/**
 * Compact build-model dropdown shared by the plan preview action strip and the
 * chat PlanFileCard. Cursor-flat: no visible text label, just the borderless
 * native select (its value — "Session default" / a model name — is self-
 * explanatory). `label` survives only as the `aria-label` for accessibility and
 * tests. The empty value means "use the session's current model"; the single
 * source of truth is the global `tomcat.plan.buildModel` config.
 */
export function PlanBuildModelSelect({
  availableModels,
  disabled = false,
  label = "Build model",
  onChange,
  testId = "plan-build-model-select",
  value,
}: {
  availableModels: string[];
  disabled?: boolean;
  label?: string;
  onChange(modelId: string): void;
  testId?: string;
  value: string;
}) {
  const hasModels = availableModels.length > 0;
  const current = value && availableModels.includes(value) ? value : "";
  return (
    <label className="tc-field tc-field--compact tc-field--dropdown tc-field--model tc-plan-model-select">
      <select
        aria-label={label}
        data-testid={testId}
        disabled={disabled || !hasModels}
        onChange={(event) => onChange(event.target.value)}
        value={current}
      >
        <option value="">{hasModels ? "Session default" : "No ready models"}</option>
        {availableModels.map((model) => (
          <option key={model} value={model}>
            {model}
          </option>
        ))}
      </select>
    </label>
  );
}
