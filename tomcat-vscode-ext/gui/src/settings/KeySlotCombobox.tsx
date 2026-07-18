import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";

import { isValidKeySlotName } from "./keySlot";

export interface KeySlotOption {
  envName: string;
  group: "saved" | "suggested";
  keyPresent: boolean;
  label: string;
}

interface KeySlotComboboxProps {
  feedback?: string | null;
  hint: string;
  onChange(nextEnvName: string): void;
  onRefresh(): void;
  options: KeySlotOption[];
  placeholder: string;
  refreshDisabled: boolean;
  refreshLabel: string;
  refreshing: boolean;
  value: string;
}

type ComboboxEntry =
  | {
      envName: string;
      group: "create";
      id: string;
      kind: "create";
      valid: boolean;
    }
  | {
      envName: string;
      group: "saved" | "suggested";
      id: string;
      kind: "option";
      keyPresent: boolean;
      label: string;
    };

function optionId(prefix: string, group: string, envName: string): string {
  return `${prefix}-${group}-${envName.replace(/[^a-zA-Z0-9_-]+/g, "-")}`;
}

function matchesQuery(option: KeySlotOption, query: string): boolean {
  const trimmed = query.trim().toLowerCase();
  if (!trimmed) {
    return true;
  }
  return (
    option.envName.toLowerCase().includes(trimmed) || option.label.toLowerCase().includes(trimmed)
  );
}

export function KeySlotCombobox({
  feedback,
  hint,
  onChange,
  onRefresh,
  options,
  placeholder,
  refreshDisabled,
  refreshLabel,
  refreshing,
  value,
}: KeySlotComboboxProps) {
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(0);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const reactId = useId().replace(/:/g, "-");
  const listboxId = `key-slot-listbox-${reactId}`;
  const hintId = `key-slot-hint-${reactId}`;

  const matchingOptions = useMemo(
    () => options.filter((option) => matchesQuery(option, value)),
    [options, value],
  );
  const hasExactMatch = useMemo(
    () => options.some((option) => option.envName === value.trim()),
    [options, value],
  );
  const createEntry = useMemo(() => {
    const trimmed = value.trim();
    if (!trimmed || hasExactMatch) {
      return null;
    }
    return {
      envName: trimmed,
      group: "create" as const,
      id: optionId(listboxId, "create", trimmed),
      kind: "create" as const,
      valid: isValidKeySlotName(trimmed),
    };
  }, [hasExactMatch, listboxId, value]);
  const suggestedOptions = useMemo(
    () => matchingOptions.filter((option) => option.group === "suggested"),
    [matchingOptions],
  );
  const savedOptions = useMemo(
    () => matchingOptions.filter((option) => option.group === "saved"),
    [matchingOptions],
  );
  const flatEntries = useMemo<ComboboxEntry[]>(() => {
    const entries: ComboboxEntry[] = [];
    if (createEntry) {
      entries.push(createEntry);
    }
    entries.push(
      ...suggestedOptions.map((option) => ({
        envName: option.envName,
        group: option.group,
        id: optionId(listboxId, option.group, option.envName),
        keyPresent: option.keyPresent,
        kind: "option" as const,
        label: option.label,
      })),
    );
    entries.push(
      ...savedOptions.map((option) => ({
        envName: option.envName,
        group: option.group,
        id: optionId(listboxId, option.group, option.envName),
        keyPresent: option.keyPresent,
        kind: "option" as const,
        label: option.label,
      })),
    );
    return entries;
  }, [createEntry, listboxId, savedOptions, suggestedOptions]);
  const activeEntry = flatEntries[activeIndex] ?? null;

  useEffect(() => {
    setActiveIndex(0);
  }, [open, value, options]);

  useEffect(() => {
    if (!open) {
      return;
    }
    const handlePointerDown = (event: MouseEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    window.addEventListener("mousedown", handlePointerDown);
    return () => {
      window.removeEventListener("mousedown", handlePointerDown);
    };
  }, [open]);

  function commitSelection(nextEnvName: string): void {
    onChange(nextEnvName);
    setOpen(false);
    window.requestAnimationFrame(() => {
      inputRef.current?.focus();
    });
  }

  function moveActive(offset: number): void {
    if (flatEntries.length === 0) {
      return;
    }
    setActiveIndex((current) => (current + offset + flatEntries.length) % flatEntries.length);
  }

  function handleKeyDown(event: ReactKeyboardEvent<HTMLInputElement>): void {
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        setOpen(true);
        moveActive(1);
        return;
      case "ArrowUp":
        event.preventDefault();
        setOpen(true);
        moveActive(-1);
        return;
      case "Enter":
        if (open && activeEntry) {
          event.preventDefault();
          if (activeEntry.kind === "create" && !activeEntry.valid) {
            return;
          }
          commitSelection(activeEntry.envName);
        }
        return;
      case "Escape":
        if (open) {
          event.preventDefault();
          setOpen(false);
        }
        return;
      default:
    }
  }

  return (
    <div className="tc-field">
      <div className="tc-field__label-row">
        <span>Key slot</span>
        <button
          aria-label={refreshLabel}
          className="tc-icon-button tc-settings-keyslot__refresh"
          disabled={refreshDisabled}
          onClick={onRefresh}
          title={refreshLabel}
          type="button"
        >
          <span
            aria-hidden="true"
            className={`codicon codicon-refresh${refreshing ? " tc-codicon-spin" : ""}`}
          />
        </button>
      </div>
      <div className="tc-settings-combobox" data-testid="settings-key-slot-box" ref={rootRef}>
        <input
          aria-activedescendant={open ? activeEntry?.id : undefined}
          aria-autocomplete="list"
          aria-controls={open ? listboxId : undefined}
          aria-expanded={open}
          aria-label="Key slot"
          aria-describedby={hintId}
          autoComplete="off"
          className="tc-settings-combobox__input"
          data-testid="settings-key-slot-input"
          onChange={(event) => {
            onChange(event.target.value);
            setOpen(true);
          }}
          onFocus={() => setOpen(true)}
          onKeyDown={handleKeyDown}
          placeholder={placeholder}
          ref={inputRef}
          role="combobox"
          value={value}
        />
        <button
          aria-label={open ? "Hide key slots" : "Show key slots"}
          className="tc-settings-combobox__toggle"
          onClick={() => {
            setOpen((current) => !current);
            window.requestAnimationFrame(() => {
              inputRef.current?.focus();
            });
          }}
          type="button"
        >
          <span
            aria-hidden="true"
            className={`codicon ${open ? "codicon-chevron-up" : "codicon-chevron-down"}`}
          />
        </button>
        {open ? (
          <div
            className="tc-session-dropdown tc-settings-combobox__dropdown"
            id={listboxId}
            role="listbox"
          >
            {createEntry ? (
              <>
                <div className="tc-session-group__header">Suggested</div>
                <button
                  aria-selected={activeEntry?.id === createEntry.id}
                  className={`tc-session-item tc-settings-combobox__item${
                    activeEntry?.id === createEntry.id ? " tc-session-item--active" : ""
                  }${createEntry.valid ? "" : " tc-settings-combobox__item--disabled"}`}
                  id={createEntry.id}
                  onClick={() => {
                    if (createEntry.valid) {
                      commitSelection(createEntry.envName);
                    }
                  }}
                  onMouseDown={(event) => event.preventDefault()}
                  role="option"
                  type="button"
                >
                  <span className="tc-session-item__title">{createEntry.envName}</span>
                  <span className="tc-settings-combobox__meta">
                    {createEntry.valid ? "Create this slot" : "Use uppercase letters, numbers, and underscores"}
                  </span>
                </button>
              </>
            ) : null}
            {suggestedOptions.length > 0 ? (
              <>
                {!createEntry ? <div className="tc-session-group__header">Suggested</div> : null}
                {suggestedOptions.map((option) => {
                  const id = optionId(listboxId, option.group, option.envName);
                  return (
                    <button
                      aria-selected={activeEntry?.id === id}
                      className={`tc-session-item tc-settings-combobox__item${
                        activeEntry?.id === id ? " tc-session-item--active" : ""
                      }`}
                      id={id}
                      key={option.envName}
                      onClick={() => commitSelection(option.envName)}
                      onMouseDown={(event) => event.preventDefault()}
                      role="option"
                      type="button"
                    >
                      <span className="tc-session-item__title">{option.envName}</span>
                      <span className="tc-settings-combobox__meta">
                        {option.keyPresent ? "Configured" : "Suggested"}
                      </span>
                    </button>
                  );
                })}
              </>
            ) : null}
            {savedOptions.length > 0 ? (
              <>
                <div className="tc-session-group__header">Saved in ~/.tomcat/assets/.env</div>
                {savedOptions.map((option) => {
                  const id = optionId(listboxId, option.group, option.envName);
                  return (
                    <button
                      aria-selected={activeEntry?.id === id}
                      className={`tc-session-item tc-settings-combobox__item${
                        activeEntry?.id === id ? " tc-session-item--active" : ""
                      }`}
                      id={id}
                      key={option.envName}
                      onClick={() => commitSelection(option.envName)}
                      onMouseDown={(event) => event.preventDefault()}
                      role="option"
                      type="button"
                    >
                      <span className="tc-session-item__title">{option.envName}</span>
                      <span className="tc-settings-combobox__meta">
                        {option.keyPresent ? "Configured" : "Missing"}
                      </span>
                    </button>
                  );
                })}
              </>
            ) : null}
            {!createEntry && suggestedOptions.length === 0 && savedOptions.length === 0 ? (
              <div className="tc-session-dropdown__empty">No matching key slots.</div>
            ) : null}
          </div>
        ) : null}
      </div>
      <small className="tc-field__hint" id={hintId}>
        {hint}
      </small>
      {feedback ? <small className="tc-field__hint tc-field__hint--success">{feedback}</small> : null}
    </div>
  );
}
