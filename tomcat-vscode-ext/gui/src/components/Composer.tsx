import type { KeyboardEvent } from "react";

import type { WebviewPlanState } from "../types";

function formatPlanStatus(planState?: WebviewPlanState | null): string | null {
  if (!planState || planState === "chat") {
    return null;
  }
  return `Plan: ${planState}`;
}

export function Composer({
  availableModels,
  canPrompt,
  contextLabel,
  modeValue,
  modelValue,
  onAddAttachment,
  onModeChange,
  onModelChange,
  onPromptChange,
  onPromptKeyDown,
  onSubmit,
  planState,
  prompt,
  promptPlaceholder,
}: {
  availableModels: string[];
  canPrompt: boolean;
  contextLabel: string;
  modeValue: "chat" | "plan";
  modelValue: string;
  onAddAttachment(): void;
  onModeChange(value: "chat" | "plan"): void;
  onModelChange(value: string): void;
  onPromptChange(value: string): void;
  onPromptKeyDown(event: KeyboardEvent<HTMLTextAreaElement>): void;
  onSubmit(): void;
  planState?: WebviewPlanState | null;
  prompt: string;
  promptPlaceholder: string;
}) {
  const planStatus = formatPlanStatus(planState);

  return (
    <section className="tc-composer" aria-label="prompt" data-testid="composer">
      <div className="tc-composer__surface">
        <textarea
          aria-label="Tomcat prompt"
          data-testid="composer-input"
          disabled={!canPrompt}
          onChange={(event) => onPromptChange(event.target.value)}
          onKeyDown={onPromptKeyDown}
          placeholder={promptPlaceholder}
          rows={4}
          value={prompt}
        />
        <div className="tc-composer__bar">
          <button
            aria-label="Add attachment"
            className="tc-icon-button"
            data-testid="attachment-add"
            disabled={!canPrompt}
            onClick={onAddAttachment}
            type="button"
          >
            +
          </button>

          <label className="tc-field tc-field--compact">
            <span>Mode</span>
            <select
              aria-label="Tomcat chat mode"
              data-testid="mode-select"
              disabled={!canPrompt}
              onChange={(event) => onModeChange(event.target.value as "chat" | "plan")}
              value={modeValue}
            >
              <option value="chat">Chat</option>
              <option value="plan">Plan</option>
            </select>
          </label>

          {planStatus ? <span className="tc-chip">{planStatus}</span> : null}

          <label className="tc-field tc-field--compact">
            <span>Model</span>
            <select
              aria-label="Tomcat model"
              data-testid="model-select"
              disabled={!canPrompt || !availableModels.length}
              onChange={(event) => onModelChange(event.target.value)}
              value={modelValue}
            >
              <option value="">Select model</option>
              {availableModels.map((model) => (
                <option key={model} value={model}>
                  {model}
                </option>
              ))}
            </select>
          </label>

          <span className="tc-composer__context" data-testid="context-ratio">
            {contextLabel}
          </span>

          <button
            aria-label="Send prompt"
            className="tc-send-button"
            data-testid="send-button"
            disabled={!prompt.trim() || !canPrompt}
            onClick={onSubmit}
            type="button"
          >
            ↑
          </button>
        </div>
      </div>
      <p className="tc-composer__hint">{promptPlaceholder}</p>
    </section>
  );
}
