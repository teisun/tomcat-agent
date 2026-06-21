import type { WebviewPlanFileRef } from "../types";

function basename(filePath: string): string {
  const normalized = filePath.replace(/\\/g, "/");
  const segments = normalized.split("/");
  return segments[segments.length - 1] || filePath;
}

function isVisiblePlanState(state: WebviewPlanFileRef["state"]): boolean {
  return state === "planning" || state === "executing" || state === "pending";
}

export function ActivePlanStrip({
  canBuild,
  onBuild,
  onOpenPlanFile,
  planFile,
}: {
  canBuild: boolean;
  onBuild(): void;
  onOpenPlanFile(path: string): void;
  planFile: WebviewPlanFileRef | null | undefined;
}) {
  if (!planFile || !isVisiblePlanState(planFile.state)) {
    return null;
  }

  return (
    <section className="tc-active-plan" data-testid="active-plan-strip">
      <button
        aria-label="Open active plan file"
        className="tc-active-plan__link"
        onClick={() => onOpenPlanFile(planFile.path)}
        type="button"
      >
        <span className="tc-active-plan__file">{basename(planFile.path)}</span>
        <span className="tc-active-plan__meta">{`Plan: ${planFile.state}`}</span>
      </button>
      <button
        aria-label="Build plan"
        className="tc-button tc-button--primary"
        data-testid="build-plan"
        disabled={!canBuild}
        onClick={onBuild}
        type="button"
      >
        Build
      </button>
    </section>
  );
}
