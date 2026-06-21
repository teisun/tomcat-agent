import type { WebviewPlanFileCard } from "../types";

function basename(filePath: string): string {
  const normalized = filePath.replace(/\\/g, "/");
  const segments = normalized.split("/");
  return segments[segments.length - 1] || filePath;
}

export function PlanFileCard({
  item,
  onOpenPlanFile,
}: {
  item: WebviewPlanFileCard;
  onOpenPlanFile(path: string): void;
}) {
  return (
    <section className="tc-card tc-plan-card" data-testid="plan-card">
      <div className="tc-card__header">
        <h3>Plan file</h3>
        <span className="tc-chip">{item.state ? `Plan: ${item.state}` : "Plan"}</span>
      </div>
      <button
        aria-label="Open plan file"
        className="tc-plan-card__link"
        onClick={() => onOpenPlanFile(item.path)}
        type="button"
      >
        <strong>{basename(item.path)}</strong>
        <span>{item.path}</span>
      </button>
    </section>
  );
}
