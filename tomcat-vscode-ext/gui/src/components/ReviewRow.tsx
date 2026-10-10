import { memo, useEffect, useState } from "react";
import { useLocale, useT } from "../i18n/LocaleProvider";
import { pluralKey, type Translator } from "../../../src/shared/i18n";

import { DisclosureCard, type DisclosureStatusVariant } from "./DisclosureCard";

import type { WebviewReviewRow } from "../types";

function verdictLabel(verdict: NonNullable<WebviewReviewRow["verdict"]>, t: Translator): string {
  if (verdict === "pass" || verdict === "fail" || verdict === "partial" || verdict === "aborted") return t(`term.verdict.${verdict}`);
  return verdict.toUpperCase();
}

function disclosureVariant(item: WebviewReviewRow): DisclosureStatusVariant {
  if (item.status === "running") return "running";
  if (item.verdict === "pass") return "success";
  if (item.verdict === "fail") return "error";
  if (item.verdict === "partial") return "warning";
  return "neutral";
}

function formatElapsed(ms: number): string {
  const totalSeconds = Math.max(0, Math.floor(ms / 1000));
  const seconds = totalSeconds % 60;
  const totalMinutes = Math.floor(totalSeconds / 60);
  const minutes = totalMinutes % 60;
  const hours = Math.floor(totalMinutes / 60);
  if (hours > 0) {
    return `${hours}:${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
  }
  return `${String(totalMinutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
}

function runningMeta(item: WebviewReviewRow, nowTick: number, t: Translator): string | null {
  const parts: string[] = [];
  const round = item.round ?? item.rounds;
  if (typeof round === "number") {
    parts.push(t("review.round", { round }));
  }
  if (typeof item.startedAt === "number") {
    parts.push(t("review.elapsed", { duration: formatElapsed(nowTick - item.startedAt) }));
  }
  return parts.length > 0 ? parts.join(" · ") : null;
}

function ReviewRowComponent({ item }: { item: WebviewReviewRow }) {
  const t = useT();
  const locale = useLocale();
  const shellClassName = "tc-tool-row-shell tc-tool-row-shell--standalone";
  const leadingIcon = (
    <span aria-hidden="true" className="tc-tool-row__leading-icon codicon codicon-shield" />
  );
  const [nowTick, setNowTick] = useState(() => Date.now());

  useEffect(() => {
    if (item.status !== "running" || typeof item.startedAt !== "number") {
      return;
    }
    const intervalId = window.setInterval(() => {
      setNowTick(Date.now());
    }, 1000);
    return () => window.clearInterval(intervalId);
  }, [item.startedAt, item.status]);

  if (item.status === "running") {
    const meta = runningMeta(item, nowTick, t);
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
                {t("review.running")}
              </span>
              {meta ? (
                <span className="tc-review-row__count" data-testid="review-row-running-meta">
                  {meta}
                </span>
              ) : null}
            </span>
          </div>
        </div>
      </div>
    );
  }

  const verdict = item.verdict ?? "aborted";
  if (verdict === "skipped") {
    return (
      <div className={shellClassName} data-testid="review-row-wrapper">
        {leadingIcon}
        <div className="tc-tool-row tc-review-row" data-testid="review-row-skipped">
          <div className="tc-tool-row__header">
            <span className="tc-review-row__count">
              {t("review.skipped", { summary: item.summary ?? t("review.skippedDefault") })}
            </span>
          </div>
        </div>
      </div>
    );
  }

  const findings = item.findings ?? [];
  const findingsLabel = t(pluralKey(locale, "review.findings.other", findings.length), { count: findings.length });
  const header = (
    <div className="tc-review-row__header" data-testid="review-row-header">
      <span className="tc-tool-row__inline">
        <span className="tc-tool-row__text">{t("review.title")}</span>
        <span
          className={`tc-review-row__badge tc-review-row__badge--${verdict}`}
          data-testid="review-row-verdict"
        >
          {verdictLabel(verdict, t)}
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
      {t("review.expand")}
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
              {t("review.roundDetail", { round: item.round ?? item.rounds ?? 0 })}
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
                    {finding.severity} · {finding.area || t("review.general")}
                  </span>
                  <span className="tc-review-row__finding-note">{finding.note}</span>
                </li>
              ))}
            </ul>
          ) : (
            <p className="tc-review-row__meta" data-testid="review-row-empty-findings">
              {t("review.empty")}
            </p>
          )}
        </div>
      </DisclosureCard>
    </div>
  );
}

export const ReviewRow = memo(
  ReviewRowComponent,
  (previous, next) => previous.item === next.item,
);
