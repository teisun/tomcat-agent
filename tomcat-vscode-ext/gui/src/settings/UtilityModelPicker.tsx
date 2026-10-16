import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";

import type { SettingsModelView } from "../../../src/shared/settingsProtocol";
import { useT } from "../i18n/LocaleProvider";
import type { ProviderCard } from "./providerCards";
import { findPresetByKeyEnv, humanizeProviderLabel } from "./providerPresets";

interface Candidate {
  cardTitle: string;
  id: string;
}

interface UtilityModelPickerProps {
  canSet: boolean;
  cards: readonly ProviderCard[];
  models: readonly SettingsModelView[];
  onSelect(modelId: string): void;
  utilityModelId: string;
}

/** Points the `llm.title_model` role at any connected model; it is a pointer, not a model. */
export function UtilityModelPicker({ canSet, cards, models, onSelect, utilityModelId }: UtilityModelPickerProps) {
  const t = useT();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const reactId = useId().replace(/:/g, "-");
  const listboxId = `utility-model-listbox-${reactId}`;
  const hintId = `utility-model-hint-${reactId}`;

  const candidates = useMemo<Candidate[]>(
    () => cards.flatMap((card) => card.rows
      .filter((row) => row.model.keyPresent)
      .map((row) => ({ cardTitle: card.title, id: row.model.id }))),
    [cards],
  );
  const matches = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return needle
      ? candidates.filter((entry) => entry.id.toLowerCase().includes(needle) || entry.cardTitle.toLowerCase().includes(needle))
      : candidates;
  }, [candidates, query]);
  const current = candidates.find((entry) => entry.id === utilityModelId) ?? null;
  const currentModel = models.find((model) => model.id === utilityModelId) ?? null;

  let problem: string | null = null;
  if (utilityModelId && !currentModel) {
    problem = t("models.utility.missing", { name: utilityModelId });
  } else if (currentModel && !currentModel.keyPresent) {
    const preset = findPresetByKeyEnv(currentModel.apiKeyEnv);
    const providerName = cards.find((card) => card.keyEnv === currentModel.apiKeyEnv)?.title
      ?? (preset ? t(preset.labelKey) : humanizeProviderLabel(currentModel.provider));
    problem = t("models.utility.notConnected", { name: providerName });
  }
  const display = !utilityModelId
    ? t("models.utility.unset")
    : current ? `${current.id} · ${current.cardTitle}` : utilityModelId;

  useEffect(() => {
    setActiveIndex(0);
  }, [open, query]);

  useEffect(() => {
    if (!open) return;
    const handlePointerDown = (event: MouseEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) close();
    };
    window.addEventListener("mousedown", handlePointerDown);
    return () => window.removeEventListener("mousedown", handlePointerDown);
  }, [open]);

  function close(): void {
    setOpen(false);
    setQuery("");
  }

  function commit(entry: Candidate | undefined): void {
    if (!entry) return;
    if (entry.id !== utilityModelId) onSelect(entry.id);
    close();
    inputRef.current?.blur();
  }

  function handleKeyDown(event: ReactKeyboardEvent<HTMLInputElement>): void {
    switch (event.key) {
      case "ArrowDown":
      case "ArrowUp": {
        event.preventDefault();
        if (!open) {
          setOpen(true);
          return;
        }
        if (matches.length === 0) return;
        const offset = event.key === "ArrowDown" ? 1 : -1;
        setActiveIndex((index) => (index + offset + matches.length) % matches.length);
        return;
      }
      case "Enter":
        if (open) {
          event.preventDefault();
          commit(matches[activeIndex]);
        }
        return;
      case "Escape":
        if (open) {
          event.preventDefault();
          event.stopPropagation();
          close();
        }
        return;
      default:
    }
  }

  const optionId = (index: number) => `${listboxId}-${index}`;
  return (
    <section className="tc-settings-utility" data-testid="settings-utility-model">
      <label className="tc-settings-utility__label" htmlFor={`${listboxId}-input`}>{t("models.utility.title")}</label>
      <div className="tc-settings-utility__control">
        <div className="tc-settings-combobox" ref={rootRef}>
          <input
            aria-activedescendant={open && matches[activeIndex] ? optionId(activeIndex) : undefined}
            aria-autocomplete="list"
            aria-controls={open ? listboxId : undefined}
            aria-describedby={hintId}
            aria-expanded={open}
            autoComplete="off"
            className="tc-settings-combobox__input"
            data-testid="settings-utility-model-input"
            disabled={!canSet}
            id={`${listboxId}-input`}
            onChange={(event) => {
              setQuery(event.target.value);
              setOpen(true);
            }}
            onFocus={() => setOpen(true)}
            onKeyDown={handleKeyDown}
            placeholder={open ? t("models.utility.search") : undefined}
            ref={inputRef}
            role="combobox"
            title={canSet ? undefined : t("models.utility.readOnly")}
            value={open ? query : display}
          />
          <span aria-hidden="true" className="tc-settings-combobox__toggle">
            <span className={`codicon ${open ? "codicon-chevron-up" : "codicon-chevron-down"}`} />
          </span>
          {open ? (
            <div className="tc-session-dropdown tc-settings-combobox__dropdown" id={listboxId} role="listbox">
              {matches.length === 0 ? (
                <div className="tc-session-dropdown__empty">{t("models.utility.none")}</div>
              ) : matches.map((entry, index) => (
                <button
                  aria-selected={index === activeIndex}
                  className={`tc-session-item tc-settings-combobox__item${index === activeIndex ? " tc-session-item--active" : ""}`}
                  data-testid={`settings-utility-option-${entry.id}`}
                  id={optionId(index)}
                  key={entry.id}
                  onClick={() => commit(entry)}
                  onMouseDown={(event) => event.preventDefault()}
                  role="option"
                  type="button"
                >
                  <span className="tc-session-item__title">{entry.id}</span>
                  <span className="tc-settings-combobox__meta">
                    {entry.cardTitle}
                    {entry.id === utilityModelId ? ` · ${t("models.utility.current")}` : ""}
                  </span>
                </button>
              ))}
            </div>
          ) : null}
        </div>
        <small className="tc-field__hint" id={hintId}>
          {open ? t("models.utility.keys") : canSet ? t("models.utility.hint") : t("models.utility.readOnly")}
        </small>
        {problem ? (
          <div className="tc-banner tc-banner--warning" data-testid="settings-utility-model-problem">{problem}</div>
        ) : null}
      </div>
    </section>
  );
}
