import { DisclosureCard, type DisclosureStatusVariant } from "./DisclosureCard";

import type { WebviewReviewRow } from "../types";

function verdictLabel(verdict: NonNullable<WebviewReviewRow["verdict"]>): string {
  return verdict.toUpperCase();
}

function disclosureVariant(item: WebviewReviewRow): DisclosureStatusVariant {
  if (item.status === "running") return "running";
  if (item.verdict === "pass") return "success";
  if (item.verdict === "fail") return "error";
  if (item.verdict === "partial") return "warning";
  return "neutral";
}

export function ReviewRow({ item }: { item: WebviewReviewRow }) {
  const shellClassName = "tc-tool-row-shell tc-tool-row-shell--standalone";
  const leadingIcon = (
    <span aria-hidden="true" className="tc-tool-row__leading-icon codicon codicon-shield" />
  );

  if (item.status === "running") {
    return (
      <div className={shellClassName} data-testid="review-row-wrapper">
        {leadingIcon}
        <div className="tc-tool-row tc-review-row tc-review-row--running" data-testid="review-row">
          <div className="tc-tool-row__header">
            <span className="tc-tool-row__label">
              <span
                className="tc-tool-row__text tc-loading-shimmer"
                data-testid="review-row-running-text"
              >
                Reviewing code...
              </span>
            </span>
          </div>
        </div>
      </div>
    );
  }

  const verdict = item.verdict ?? "aborted";
  const findings = item.findings ?? [];
  const findingsLabel = `${findings.length} finding${findings.length === 1 ? "" : "s"}`;
  const header = (
    <div className="tc-review-row__header" data-testid="review-row-header">
      <span className="tc-tool-row__inline">
        <span className="tc-tool-row__text">Code review</span>
        <span
          className={`tc-review-row__badge tc-review-row__badge--${verdict}`}
          data-testid="review-row-verdict"
        >
          {verdictLabel(verdict)}
        </span>
        <span className="tc-review-row__count" data-testid="review-row-findings-count">
          {findingsLabel}
        </span>
      </span>
    </div>
  );
  const preview = item.summary ? (
    <p className="tc-review-row__summary" data-testid="review-row-preview">
      {item.summary}
    </p>
  ) : (
    <p className="tc-review-row__summary" data-testid="review-row-preview">
      Expand to inspect review details.
    </p>
  );

  return (
    <div className={shellClassName} data-testid="review-row-wrapper">
      <DisclosureCard
        bodyTestId="review-row-body"
        header={header}
        leadingIcon={leadingIcon}
        preview={preview}
        resetKey={item.id}
        statusVariant={disclosureVariant(item)}
        toggleTestId="review-row-toggle"
      >
        <div className="tc-review-row__details">
          {item.round ?? item.rounds ? (
            <p className="tc-review-row__meta" data-testid="review-row-rounds">
              Review round {item.round ?? item.rounds}
            </p>
          ) : null}
          {item.summary ? (
            <p className="tc-review-row__summary" data-testid="review-row-summary">
              {item.summary}
            </p>
          ) : null}
          {findings.length > 0 ? (
            <ul className="tc-review-row__findings" data-testid="review-row-findings">
              {findings.map((finding, index) => (
                <li
                  className="tc-review-row__finding"
                  data-testid="review-row-finding"
                  key={`${item.id}-finding-${index}`}
                >
                  <span className="tc-review-row__finding-meta">
                    {finding.severity} · {finding.area || "general"}
                  </span>
                  <span className="tc-review-row__finding-note">{finding.note}</span>
                </li>
              ))}
            </ul>
          ) : (
            <p className="tc-review-row__meta" data-testid="review-row-empty-findings">
              No structured findings were returned.
            </p>
          )}
        </div>
      </DisclosureCard>
    </div>
  );
}
