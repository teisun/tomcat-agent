import {
  type ReactNode,
  type RefObject,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";

import { SPEEDS, type Speed } from "../../../src/shared/modelSpeed";
import { filterModels } from "./modelSearch";
import { formatModelLabel, modelLabelParts, thinkingLevelLabel } from "./modelLabel";

export interface ModelPickerModel {
  capabilities?: readonly string[];
  contextWindow?: number | null;
  contextWindowOptions?: readonly number[];
  description?: string | null;
  id: string;
  modelName?: string | null;
  selectedContextWindow?: number | null;
  selectedReasoningLevel?: string | null;
  supportedReasoningLevels?: readonly string[];
  supportedSpeeds?: readonly Speed[];
  selectedSpeed?: Speed | null;
}

export interface ModelPickerProps {
  className?: string;
  disabled?: boolean;
  dropdownTestId?: string;
  /** Invisible compatibility trigger for the existing VS Code host E2E contract. */
  legacyThinkingTriggerTestId?: string;
  label?: string;
  models: readonly ModelPickerModel[];
  placement?: "above" | "below";
  onOpenModelSettings?: () => void;
  onSelectContextWindow?: (modelId: string, contextWindow: number) => void;
  onSelectModel: (modelId: string) => void;
  onSelectThinkingLevel?: (modelId: string, level: string) => void;
  onSelectSpeed?: (modelId: string, speed: Speed) => void;
  optionTestId?: string;
  selectedModelId: string | null | undefined;
  testId?: string;
}

type ConfigPopoverPlacement = "left" | "right";

type ConfigPopoverPosition = {
  left: number;
  placement: ConfigPopoverPlacement;
  top: number;
};

function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), Math.max(min, max));
}

/**
 * A compact model control shared by chat and plan surfaces.
 *
 * Every caller gets one interaction model: pick a row to switch models, or
 * use its Edit action to choose Context and Effort.
 */
export function ModelPicker({
  className,
  disabled = false,
  dropdownTestId = "model-dropdown",
  legacyThinkingTriggerTestId,
  label = "Model",
  models,
  onOpenModelSettings,
  onSelectContextWindow,
  onSelectModel,
  onSelectThinkingLevel,
  onSelectSpeed,
  optionTestId = "model-option",
  placement = "above",
  selectedModelId,
  testId = "model-select",
}: ModelPickerProps) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [configModelId, setConfigModelId] = useState<string | null>(null);
  const [configPosition, setConfigPosition] = useState<ConfigPopoverPosition | null>(null);
  const [hoveredModelId, setHoveredModelId] = useState<string | null>(null);
  const [focusedModelId, setFocusedModelId] = useState<string | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const primaryPopoverRef = useRef<HTMLDivElement>(null);
  const configPopoverRef = useRef<HTMLElement>(null);
  const optionRefs = useRef<Record<string, HTMLDivElement | null>>({});
  const closeConfigTimer = useRef<number | null>(null);

  const visibleModels = useMemo(() => filterModels(models, query), [models, query]);
  const selectedModel =
    models.find((model) => model.id === selectedModelId) ?? null;
  const triggerLabel = formatModelLabel({
    modelId: selectedModelId,
    selectedReasoningLevel: selectedModel?.selectedReasoningLevel,
    supportedReasoningLevels: selectedModel?.supportedReasoningLevels,
  });
  const configModel =
    models.find((model) => model.id === configModelId) ?? null;

  useEffect(() => {
    if (!open || !selectedModelId) {
      return;
    }
    const selectedOption = optionRefs.current[selectedModelId];
    if (typeof selectedOption?.scrollIntoView === "function") {
      selectedOption.scrollIntoView({ block: "nearest" });
    }
  }, [open, selectedModelId]);

  const clearCloseTimer = () => {
    if (closeConfigTimer.current !== null) {
      window.clearTimeout(closeConfigTimer.current);
      closeConfigTimer.current = null;
    }
  };

  const openConfig = (model: ModelPickerModel) => {
    clearCloseTimer();
    setConfigModelId(model.id);
  };

  const scheduleConfigClose = () => {
    clearCloseTimer();
    closeConfigTimer.current = window.setTimeout(() => {
      setConfigModelId(null);
      closeConfigTimer.current = null;
    }, 150);
  };

  useLayoutEffect(() => {
    if (!configModelId) {
      setConfigPosition(null);
      return;
    }
    const updatePosition = () => {
      const row = optionRefs.current[configModelId];
      const configPopover = configPopoverRef.current;
      if (!row || !configPopover) {
        return;
      }

      const anchorRect = row.getBoundingClientRect();
      const configRect = configPopover.getBoundingClientRect();
      const configWidth = configRect.width || configPopover.offsetWidth || 200;
      const configHeight = configRect.height || configPopover.offsetHeight || 176;
      const gap = 8;
      const viewportPadding = 8;
      const rightSpace = window.innerWidth - anchorRect.right;
      const leftSpace = anchorRect.left;
      const placement: ConfigPopoverPlacement =
        rightSpace >= configWidth + gap || rightSpace >= leftSpace ? "right" : "left";
      const rawLeft =
        placement === "right"
          ? anchorRect.right + gap
          : anchorRect.left - gap - configWidth;
      const centeredTop = anchorRect.top + anchorRect.height / 2 - configHeight / 2;

      setConfigPosition({
        left: clamp(
          rawLeft,
          viewportPadding,
          window.innerWidth - configWidth - viewportPadding,
        ),
        placement,
        top: clamp(
          centeredTop,
          viewportPadding,
          window.innerHeight - configHeight - viewportPadding,
        ),
      });
    };

    updatePosition();
    window.addEventListener("resize", updatePosition);
    // A transcript scroll moves the anchor under a fixed portal. Closing is
    // less surprising than leaving the configuration menu detached from its row.
    const closeOnScroll = () => setConfigModelId(null);
    document.addEventListener("scroll", closeOnScroll, true);
    return () => {
      window.removeEventListener("resize", updatePosition);
      document.removeEventListener("scroll", closeOnScroll, true);
    };
  }, [configModelId, visibleModels]);

  useEffect(
    () => () => {
      clearCloseTimer();
    },
    [],
  );

  useEffect(() => {
    if (!open) return;
    const closeFromOutside = (event: MouseEvent) => {
      const target = event.target;
      if (!(target instanceof Node)) {
        setConfigModelId(null);
        setOpen(false);
        return;
      }
      // A portal is outside rootRef, so it must be checked first. Keeping it
      // open through mousedown lets the nested option receive its later click.
      if (configPopoverRef.current?.contains(target)) {
        return;
      }
      if (rootRef.current?.contains(target) || primaryPopoverRef.current?.contains(target)) {
        setConfigModelId(null);
        return;
      }
      setConfigModelId(null);
      setOpen(false);
    };
    const closeFromEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      if (configModelId) {
        setConfigModelId(null);
      } else {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", closeFromOutside);
    document.addEventListener("keydown", closeFromEscape);
    return () => {
      document.removeEventListener("mousedown", closeFromOutside);
      document.removeEventListener("keydown", closeFromEscape);
    };
  }, [configModelId, open]);

  function selectModel(modelId: string) {
    onSelectModel(modelId);
    setOpen(false);
    setQuery("");
    setConfigModelId(null);
  }

  return (
    <div
      className={["tc-model-picker", className].filter(Boolean).join(" ")}
      ref={rootRef}
    >
      <button
        aria-expanded={open}
        aria-haspopup="dialog"
        aria-label={label}
        className={[
          "tc-topbar__trigger",
          "tc-topbar__trigger--compact",
          "tc-model-picker-trigger",
        ].join(" ")}
        data-testid={testId}
        disabled={disabled || models.length === 0}
        onClick={() => {
          setOpen((value) => {
            const nextOpen = !value;
            if (!nextOpen) {
              setConfigModelId(null);
              setQuery("");
              clearCloseTimer();
            }
            return nextOpen;
          });
        }}
        type="button"
      >
        <span className="tc-model-picker-trigger-label">{triggerLabel}</span>
        <i
          aria-hidden="true"
          className={`codicon codicon-chevron-${placement === "below" ? "down" : "up"}`}
        />
      </button>
      {legacyThinkingTriggerTestId &&
      selectedModel ? (
        <button
          aria-hidden="true"
          className="tc-model-picker-legacy-thinking-trigger"
          data-testid={legacyThinkingTriggerTestId}
          onClick={() => {
            setOpen(true);
            setConfigModelId(selectedModel.id);
          }}
          tabIndex={-1}
          type="button"
        >
          <span>
            {formatModelLabel({
              modelId: selectedModel.id,
              selectedReasoningLevel: selectedModel.selectedReasoningLevel,
              supportedReasoningLevels: selectedModel.supportedReasoningLevels,
            })}
          </span>
        </button>
      ) : null}
      {open ? (
        <div
          className={`tc-session-dropdown tc-model-picker-dropdown tc-model-picker-dropdown--${placement}`}
          data-testid={dropdownTestId}
          ref={primaryPopoverRef}
        >
          <input
            aria-label="Search models"
            autoFocus
            className="tc-model-picker-search"
            onChange={(event) => {
              setConfigModelId(null);
              setQuery(event.target.value);
            }}
            placeholder="Search models"
            type="search"
            value={query}
          />
          <div className="tc-model-picker-list">
            {visibleModels.length === 0 ? (
              <div className="tc-model-picker-empty">No matching models</div>
            ) : (
              visibleModels.map((model) => {
                const selected = model.id === selectedModelId;
                const editVisible =
                  model.id === hoveredModelId ||
                  model.id === focusedModelId ||
                  model.id === configModelId;
                const label = modelLabelParts({
                  modelId: model.id,
                  selectedReasoningLevel: model.selectedReasoningLevel,
                  supportedReasoningLevels: model.supportedReasoningLevels,
                });
                return (
                  <div
                    data-model-id={model.id}
                    ref={(node) => {
                      optionRefs.current[model.id] = node;
                    }}
                    className={[
                      "tc-model-picker-option",
                      selected ? "is-selected" : "",
                      configModelId === model.id ? "is-configuring" : "",
                    ]
                      .filter(Boolean)
                      .join(" ")}
                    key={model.id}
                    onBlur={(event) => {
                      const nextFocused = event.relatedTarget;
                      if (!(nextFocused instanceof Node) || !event.currentTarget.contains(nextFocused)) {
                        setFocusedModelId(null);
                      }
                    }}
                    onFocus={() => setFocusedModelId(model.id)}
                    onMouseEnter={() => {
                      clearCloseTimer();
                      setHoveredModelId(model.id);
                    }}
                    onMouseLeave={() => {
                      setHoveredModelId(null);
                      scheduleConfigClose();
                    }}
                  >
                    <button
                      aria-current={selected ? "true" : undefined}
                      className="tc-session-item tc-model-picker-option-select"
                      data-testid={optionTestId}
                      onClick={() => selectModel(model.id)}
                      type="button"
                    >
                      <span className="tc-model-picker-option-label">
                        <span className="tc-model-picker-option-id">{label.id}</span>
                        {label.reasoning ? (
                          <span className="tc-model-picker-option-effort">{label.reasoning}</span>
                        ) : null}
                      </span>
                    </button>
                    <span className="tc-model-picker-option-action">
                      <button
                        aria-hidden={!editVisible}
                        aria-label={`Edit ${model.modelName ?? model.id}`}
                        className={[
                          "tc-model-picker-edit",
                          editVisible ? "is-visible" : "",
                        ]
                          .filter(Boolean)
                          .join(" ")}
                        data-testid={`model-edit-${model.id}`}
                        onClick={(event) => {
                          event.stopPropagation();
                          openConfig(model);
                        }}
                        tabIndex={editVisible ? 0 : -1}
                        type="button"
                      >
                        Edit
                      </button>
                      {selected && !editVisible ? (
                        <i
                          aria-label="Selected"
                          className="codicon codicon-check"
                        />
                      ) : null}
                    </span>
                  </div>
                );
              })
            )}
          </div>
          {onOpenModelSettings ? (
            <button
              className="tc-model-picker-settings"
              data-testid="model-open-settings"
              onClick={() => {
                setConfigModelId(null);
                setOpen(false);
                onOpenModelSettings();
              }}
              type="button"
            >
              Add Models...
            </button>
          ) : null}
        </div>
      ) : null}
      {configModel && typeof document !== "undefined"
        ? createPortal(
            <ModelConfigPopover
              onMouseEnter={clearCloseTimer}
              onMouseLeave={scheduleConfigClose}
              model={configModel}
              onSelectContextWindow={onSelectContextWindow}
              onSelectThinkingLevel={onSelectThinkingLevel}
              onSelectSpeed={onSelectSpeed}
              placement={configPosition?.placement ?? "right"}
              popoverRef={configPopoverRef}
              position={configPosition}
            />,
            document.body,
          )
        : null}
    </div>
  );
}

function ModelConfigPopover({
  model,
  onMouseEnter,
  onMouseLeave,
  onSelectContextWindow,
  onSelectThinkingLevel,
  onSelectSpeed,
  placement,
  popoverRef,
  position,
}: Pick<ModelPickerProps, "onSelectContextWindow" | "onSelectThinkingLevel" | "onSelectSpeed"> & {
  model: ModelPickerModel;
  onMouseEnter: () => void;
  onMouseLeave: () => void;
  placement: ConfigPopoverPlacement;
  popoverRef: RefObject<HTMLElement | null>;
  position: ConfigPopoverPosition | null;
}) {
  const contextWindowOptions =
    model.contextWindowOptions?.length
      ? model.contextWindowOptions
      : model.contextWindow === null || model.contextWindow === undefined
        ? []
        : [model.contextWindow];
  const supportedReasoningLevels = model.supportedReasoningLevels ?? [];
  const selectedContextWindow =
    model.selectedContextWindow ?? model.contextWindow;
  return (
    <section
      aria-label={`Configure ${model.modelName ?? model.id}`}
      className={`tc-model-config-popover is-side-${placement}`}
      data-testid="thinking-level-dropdown"
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
      ref={popoverRef}
      style={{
        left: position?.left ?? -10_000,
        top: position?.top ?? -10_000,
        visibility: position ? "visible" : "hidden",
      }}
    >
      {onSelectSpeed && (model.supportedSpeeds?.length ?? 0) > 0 ? (
        <ConfigSection title="Speed">
          {SPEEDS.filter(
            (speed) => speed === "standard" || model.supportedSpeeds?.includes(speed),
          ).map((speed) => (
            <ConfigOption
              key={speed}
              label={{ standard: "Standard", fast: "Fast", ultrafast: "Ultrafast" }[speed]}
              onSelect={() => onSelectSpeed(model.id, speed)}
              selected={speed === (model.selectedSpeed ?? "standard")}
              testId="speed-option"
              title={speed === "standard"
                ? undefined
                : "Faster output; higher cost. Upstream access required."}
            />
          ))}
        </ConfigSection>
      ) : null}
      {onSelectContextWindow && contextWindowOptions.length > 0 ? (
        <ConfigSection title="Context">
          {contextWindowOptions.map((contextWindow) => (
            <ConfigOption
              key={contextWindow}
              label={formatContextWindow(contextWindow)}
              onSelect={() => onSelectContextWindow?.(model.id, contextWindow)}
              selected={contextWindow === selectedContextWindow}
              testId="context-window-option"
            />
          ))}
        </ConfigSection>
      ) : null}
      {onSelectThinkingLevel && supportedReasoningLevels.length > 0 ? (
        <ConfigSection title="Effort">
          {supportedReasoningLevels.map((level) => (
            <ConfigOption
              key={level}
              label={thinkingLevelLabel(level)}
              onSelect={() => onSelectThinkingLevel?.(model.id, level)}
              selected={level === model.selectedReasoningLevel}
              testId="thinking-level-option"
            />
          ))}
        </ConfigSection>
      ) : null}
    </section>
  );
}

function ConfigSection({
  children,
  title,
}: {
  children: ReactNode;
  title: string;
}) {
  return (
    <section className="tc-model-config-section">
      <h3>{title}</h3>
      {children}
    </section>
  );
}

function formatContextWindow(contextWindow: number): string {
  if (contextWindow >= 1_000_000) {
    return `${contextWindow / 1_000_000}M`;
  }
  const thousands = contextWindow / 1_000;
  return `${Number.isInteger(thousands) ? thousands : thousands.toFixed(1)}K`;
}

function ConfigOption({
  label,
  onSelect,
  selected,
  testId,
  title,
}: {
  label: string;
  onSelect: () => void;
  selected: boolean;
  testId: string;
  title?: string;
}) {
  return (
    <button
      className="tc-model-config-option"
      aria-label={label}
      aria-pressed={selected}
      title={title}
      data-testid={testId}
      onClick={onSelect}
      type="button"
    >
      <span>{label}</span>
      {selected ? (
        <i aria-label="Selected" className="codicon codicon-check" />
      ) : null}
    </button>
  );
}
