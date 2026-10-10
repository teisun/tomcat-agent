import { useT } from "../i18n/LocaleProvider";
import { buildReferenceTitle } from "../contextReferences";
import type { WebviewReference } from "../types";

export function ReferenceChip({
  onRemove,
  reference,
  testId = "reference-chip",
}: {
  onRemove?(): void;
  reference: WebviewReference;
  testId?: string;
}) {
  const t = useT();
  const title = buildReferenceTitle(reference);
  const iconClass =
    reference.kind === "selection" ? "codicon codicon-symbol-snippet" : "codicon codicon-file";

  return (
    <span
      aria-label={title}
      aria-roledescription={t("reference.role")}
      className="tc-chip tc-chip--reference"
      data-ref-kind={reference.kind}
      data-testid={testId}
      role="group"
      title={title}
    >
      <span aria-hidden="true" className={iconClass} />
      <span className="tc-chip__label">{reference.label}</span>
      {onRemove ? (
        <button
          aria-label={t("reference.remove", { name: reference.label })}
          className="tc-chip__remove"
          data-testid={`${testId}-remove`}
          onClick={onRemove}
          type="button"
        >
          <span aria-hidden="true" className="codicon codicon-close" />
        </button>
      ) : null}
    </span>
  );
}
