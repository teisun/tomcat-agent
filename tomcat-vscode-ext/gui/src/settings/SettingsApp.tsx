import { useEffect, useMemo, useState } from "react";

import type {
  SettingsHostFrame,
  SettingsIntent,
  SettingsModelCapabilities,
  SettingsModelInput,
  SettingsModelView,
  SettingsStateSnapshot,
  VsCodeApiLike,
} from "../../../src/shared/settingsProtocol";

type FormState = SettingsModelInput;
type FormMode = "create" | "edit";

const EMPTY_CAPABILITIES: SettingsModelCapabilities = {
  files: false,
  reasoning: true,
  tools: true,
  vision: false,
  webSearch: false,
};

function createEmptyForm(): FormState {
  return {
    api: "openai",
    apiKeyEnv: "",
    baseUrl: "",
    capabilities: { ...EMPTY_CAPABILITIES },
    contextWindow: null,
    id: "",
    modelName: "",
    provider: "",
    thinkingFormat: "",
  };
}

function inferApiKeyEnv(provider: string): string {
  const normalized = provider
    .trim()
    .replace(/[^A-Za-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "")
    .toUpperCase();
  return normalized ? `${normalized}_API_KEY` : "";
}

function normalizeModel(model: FormState): SettingsModelInput {
  return {
    api: model.api.trim(),
    apiKeyEnv: model.apiKeyEnv?.trim() ? model.apiKeyEnv.trim() : null,
    baseUrl: model.baseUrl?.trim() ? model.baseUrl.trim() : null,
    capabilities: { ...model.capabilities },
    contextWindow:
      typeof model.contextWindow === "number" && Number.isFinite(model.contextWindow)
        ? model.contextWindow
        : null,
    id: model.id.trim(),
    modelName: model.modelName?.trim() ? model.modelName.trim() : null,
    provider: model.provider.trim(),
    thinkingFormat: model.thinkingFormat?.trim() ? model.thinkingFormat.trim() : null,
  };
}

function frameIsState(message: unknown): message is SettingsHostFrame {
  return (
    typeof message === "object" &&
    message !== null &&
    (message as { channel?: unknown }).channel === "state"
  );
}

function modelToForm(model: SettingsModelView): FormState {
  return {
    api: model.api,
    apiKeyEnv: model.apiKeyEnv,
    baseUrl: model.baseUrl ?? "",
    capabilities: { ...model.capabilities },
    contextWindow: model.contextWindow ?? null,
    id: model.id,
    modelName: model.modelName ?? "",
    provider: model.provider,
    thinkingFormat: model.thinkingFormat ?? "",
  };
}

function modelKeyEnvName(model: SettingsModelView): string {
  return model.apiKeyEnv?.trim() || inferApiKeyEnv(model.provider);
}

function buildModelDetails(model: SettingsModelView): Array<{ label: string; value: string }> {
  const detailRows = [
    {
      label: "Source",
      value: model.source === "user" ? "User" : "Built-in",
    },
    {
      label: "API",
      value: model.api,
    },
    {
      label: "Provider",
      value: model.provider,
    },
    {
      label: "API Key Env",
      value: modelKeyEnvName(model),
    },
    {
      label: "Base URL",
      value: model.baseUrl ?? "",
    },
    {
      label: "Thinking",
      value: model.thinkingFormat ?? "",
    },
    {
      label: "Context Window",
      value:
        typeof model.contextWindow === "number"
          ? String(model.contextWindow)
          : "",
    },
    {
      label: "Upstream Model",
      value: model.modelName ?? "",
    },
  ];
  return detailRows.filter((entry) => entry.value.trim().length > 0);
}

function send(vscodeApi: VsCodeApiLike<SettingsIntent>, message: Omit<SettingsIntent, "messageId">): void {
  vscodeApi.postMessage({
    ...message,
    messageId: `${message.type}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
  } as SettingsIntent);
}

export function SettingsApp({
  vscodeApi,
}: {
  vscodeApi: VsCodeApiLike<SettingsIntent>;
}) {
  const [state, setState] = useState<SettingsStateSnapshot>({
    capabilities: {
      listModels: false,
      listProviderKeys: false,
      removeModel: false,
      setProviderKey: false,
      upsertModel: false,
    },
    models: [],
    providerKeys: [],
    ready: false,
    route: "models",
  });
  const [form, setForm] = useState<FormState>(() => createEmptyForm());
  const [draftApiKey, setDraftApiKey] = useState("");
  const [inlineApiKeys, setInlineApiKeys] = useState<Record<string, string>>({});
  const [selectedModelId, setSelectedModelId] = useState<string | null>(null);
  const [formMode, setFormMode] = useState<FormMode>("create");
  const [isFormOpen, setIsFormOpen] = useState(false);
  const [validationError, setValidationError] = useState<string | null>(null);

  useEffect(() => {
    const handleMessage = (event: MessageEvent<unknown>) => {
      if (!frameIsState(event.data)) {
        return;
      }
      setState(event.data.content);
    };
    window.addEventListener("message", handleMessage);
    send(vscodeApi, {
      data: {
        route: "models",
      },
      type: "settings.ready",
    });
    return () => {
      window.removeEventListener("message", handleMessage);
    };
  }, [vscodeApi]);

  useEffect(() => {
    if (!isFormOpen) {
      return;
    }
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        setIsFormOpen(false);
        setFormMode("create");
        setSelectedModelId(null);
        setDraftApiKey("");
        setValidationError(null);
        setForm(createEmptyForm());
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, [isFormOpen]);

  const readyModels = useMemo(
    () => state.models.filter((model) => model.keyPresent),
    [state.models],
  );
  const needsKeyModels = useMemo(
    () => state.models.filter((model) => !model.keyPresent),
    [state.models],
  );
  const selectedModel = useMemo(
    () => state.models.find((model) => model.id === selectedModelId) ?? null,
    [selectedModelId, state.models],
  );

  const resetForm = () => {
    setSelectedModelId(null);
    setDraftApiKey("");
    setValidationError(null);
    setForm(createEmptyForm());
  };

  const closeForm = () => {
    setIsFormOpen(false);
    setFormMode("create");
    resetForm();
  };

  const openCreateForm = () => {
    resetForm();
    setFormMode("create");
    setIsFormOpen(true);
  };

  const loadModel = (model: SettingsModelView) => {
    setSelectedModelId(model.id);
    setDraftApiKey("");
    setValidationError(null);
    setForm(modelToForm(model));
  };

  const openEditForm = (model: SettingsModelView) => {
    loadModel(model);
    setFormMode("edit");
    setIsFormOpen(true);
  };

  const handleSave = () => {
    const normalized = normalizeModel(form);
    if (!normalized.id || !normalized.provider || !normalized.api) {
      setValidationError("Model ID、Provider 和 API 都必须填写。");
      return;
    }
    if (draftApiKey.trim() && !state.capabilities.setProviderKey) {
      setValidationError("当前后端不支持保存 API Key，请先升级 `tomcat serve`。");
      return;
    }
    setValidationError(null);
    const envName =
      normalized.apiKeyEnv?.trim() || inferApiKeyEnv(normalized.provider);
    send(vscodeApi, {
      data: {
        model: {
          ...normalized,
          apiKeyEnv: normalized.apiKeyEnv || null,
        },
        providerKey:
          draftApiKey.trim() && envName
            ? {
                envName,
                value: draftApiKey.trim(),
              }
            : undefined,
      },
      type: "upsertModel",
    });
    closeForm();
  };

  const handleDelete = () => {
    if (!selectedModel || selectedModel.source !== "user") {
      return;
    }
    send(vscodeApi, {
      data: {
        modelId: selectedModel.id,
      },
      type: "removeModel",
    });
    closeForm();
  };

  const handleInlineSave = (model: SettingsModelView) => {
    const value = inlineApiKeys[model.id]?.trim() ?? "";
    if (!value) {
      return;
    }
    send(vscodeApi, {
      data: {
        envName: model.apiKeyEnv || inferApiKeyEnv(model.provider),
        value,
      },
      type: "setProviderKey",
    });
    setInlineApiKeys((current) => ({
      ...current,
      [model.id]: "",
    }));
  };

  const formTitle =
    formMode === "edit" && selectedModel ? `Edit ${selectedModel.id}` : "Add Model";
  const formDescription =
    formMode === "edit"
      ? "Update this model or replace the key without exposing it in the UI."
      : "Add a custom model or override a built-in preset.";

  return (
    <div className="tc-settings-shell">
      <aside className="tc-settings-shell__nav">
        <div className="tc-settings-shell__brand">Tomcat Settings</div>
        <button
          className="tc-settings-nav__item tc-settings-nav__item--active"
          type="button"
        >
          Models
        </button>
        <button className="tc-settings-nav__item" disabled type="button">
          Sessions
        </button>
        <button className="tc-settings-nav__item" disabled type="button">
          Tools
        </button>
      </aside>
      <main className="tc-settings-shell__content">
        <header className="tc-settings-shell__header">
          <div>
            <h1>Models</h1>
            <p>
              Built-in and custom models, with provider keys stored without
              echoing them back into the UI.
            </p>
          </div>
          <div className="tc-button-row">
            <button
              className="tc-button tc-button--secondary"
              disabled={!state.capabilities.upsertModel}
              onClick={openCreateForm}
              type="button"
            >
              + Add Model
            </button>
          </div>
        </header>

        {state.error ? <div className="tc-banner tc-banner--warning">{state.error}</div> : null}
        {state.status ? <div className="tc-banner">{state.status}</div> : null}

        {!state.capabilities.listModels ? (
          <section className="tc-empty-state">
            <h2>Model management unavailable</h2>
            <p>
              The connected `tomcat serve` does not expose model management yet.
            </p>
          </section>
        ) : (
          <div className="tc-settings-groups">
            <section className="tc-settings-group">
              <h2 className="tc-settings-group__title">Ready</h2>
              {readyModels.length === 0 ? (
                <div className="tc-session-dropdown__empty">No ready models yet.</div>
              ) : (
                readyModels.map((model) => {
                  const details = buildModelDetails(model);
                  return (
                    <article className="tc-settings-model" key={model.id}>
                      <div className="tc-settings-model__header">
                        <div className="tc-settings-model__identity">
                          <span
                            aria-label="Ready"
                            className="tc-settings-model__status-dot tc-settings-model__status-dot--ready"
                            role="img"
                          />
                          <strong>{model.id}</strong>
                        </div>
                        <div className="tc-settings-model__actions">
                          <div className="tc-settings-model__tooltip-anchor">
                            <button
                              aria-label={`Show details for ${model.id}`}
                              className="tc-settings-model__info"
                              type="button"
                            >
                              <span
                                aria-hidden="true"
                                className="codicon codicon-info"
                              />
                            </button>
                            <div
                              className="tc-settings-model__tooltip"
                              role="tooltip"
                            >
                              <dl className="tc-settings-model__tooltip-list">
                                {details.map((entry) => (
                                  <div
                                    className="tc-settings-model__tooltip-row"
                                    key={`${model.id}-${entry.label}`}
                                  >
                                    <dt>{entry.label}</dt>
                                    <dd>{entry.value}</dd>
                                  </div>
                                ))}
                              </dl>
                            </div>
                          </div>
                          <button
                            className="tc-button tc-button--secondary"
                            onClick={() => openEditForm(model)}
                            type="button"
                          >
                            Edit
                          </button>
                        </div>
                      </div>
                    </article>
                  );
                })
              )}
            </section>

            <section className="tc-settings-group">
              <h2 className="tc-settings-group__title">Needs API Key</h2>
              {needsKeyModels.length === 0 ? (
                <div className="tc-session-dropdown__empty">
                  All visible models are ready.
                </div>
              ) : (
                needsKeyModels.map((model) => {
                  const details = buildModelDetails(model);
                  return (
                    <article className="tc-settings-model" key={model.id}>
                      <div className="tc-settings-model__header">
                        <div className="tc-settings-model__identity">
                          <span
                            aria-label="Needs API key"
                            className="tc-settings-model__status-dot tc-settings-model__status-dot--missing"
                            role="img"
                          />
                          <strong>{model.id}</strong>
                        </div>
                        <div className="tc-settings-model__actions">
                          <div className="tc-settings-model__tooltip-anchor">
                            <button
                              aria-label={`Show details for ${model.id}`}
                              className="tc-settings-model__info"
                              type="button"
                            >
                              <span
                                aria-hidden="true"
                                className="codicon codicon-info"
                              />
                            </button>
                            <div
                              className="tc-settings-model__tooltip"
                              role="tooltip"
                            >
                              <dl className="tc-settings-model__tooltip-list">
                                {details.map((entry) => (
                                  <div
                                    className="tc-settings-model__tooltip-row"
                                    key={`${model.id}-${entry.label}`}
                                  >
                                    <dt>{entry.label}</dt>
                                    <dd>{entry.value}</dd>
                                  </div>
                                ))}
                              </dl>
                            </div>
                          </div>
                          <button
                            className="tc-button tc-button--secondary"
                            onClick={() => openEditForm(model)}
                            type="button"
                          >
                            Edit
                          </button>
                        </div>
                      </div>
                      <div className="tc-settings-inline-key">
                        <input
                          autoComplete="off"
                          className="tc-input"
                          onChange={(event) =>
                            setInlineApiKeys((current) => ({
                              ...current,
                              [model.id]: event.target.value,
                            }))
                          }
                          placeholder={`Save ${modelKeyEnvName(model)}`}
                          type="password"
                          value={inlineApiKeys[model.id] ?? ""}
                        />
                        <button
                          className="tc-button tc-button--primary"
                          disabled={!state.capabilities.setProviderKey}
                          onClick={() => handleInlineSave(model)}
                          type="button"
                        >
                          Save
                        </button>
                      </div>
                    </article>
                  );
                })
              )}
            </section>
          </div>
        )}

        {isFormOpen ? (
          <div className="tc-settings-modal" onClick={closeForm}>
            <section
              aria-labelledby="settings-model-form-title"
              aria-modal="true"
              className="tc-card tc-settings-modal__card"
              onClick={(event) => event.stopPropagation()}
              role="dialog"
            >
              <div className="tc-settings-modal__header">
                <div>
                  <h3 id="settings-model-form-title">{formTitle}</h3>
                  <p>{formDescription}</p>
                </div>
                <button
                  aria-label="Close model form"
                  className="tc-icon-button tc-settings-modal__close"
                  onClick={closeForm}
                  type="button"
                >
                  <span aria-hidden="true" className="codicon codicon-close" />
                </button>
              </div>

              {validationError ? (
                <div className="tc-banner tc-banner--warning">{validationError}</div>
              ) : null}

              <div className="tc-settings-form">
                <label className="tc-field">
                  <span>Model ID</span>
                  <input
                    className="tc-input"
                    disabled={selectedModel !== null}
                    onChange={(event) =>
                      setForm((current) => ({ ...current, id: event.target.value }))
                    }
                    placeholder="claude-opus-gateway"
                    value={form.id}
                  />
                </label>
                <label className="tc-field">
                  <span>Model Name</span>
                  <input
                    className="tc-input"
                    onChange={(event) =>
                      setForm((current) => ({ ...current, modelName: event.target.value }))
                    }
                    placeholder="Upstream model name"
                    value={form.modelName ?? ""}
                  />
                </label>
                <div className="tc-settings-form__row">
                  <label className="tc-field">
                    <span>API</span>
                    <select
                      onChange={(event) =>
                        setForm((current) => ({ ...current, api: event.target.value }))
                      }
                      value={form.api}
                    >
                      <option value="openai">openai</option>
                      <option value="openai-responses">openai-responses</option>
                      <option value="anthropic-messages">anthropic-messages</option>
                    </select>
                  </label>
                  <label className="tc-field">
                    <span>Thinking Format</span>
                    <select
                      onChange={(event) =>
                        setForm((current) => ({
                          ...current,
                          thinkingFormat: event.target.value,
                        }))
                      }
                      value={form.thinkingFormat ?? ""}
                    >
                      <option value="">Auto</option>
                      <option value="openai">openai</option>
                      <option value="deepseek">deepseek</option>
                      <option value="zai">zai</option>
                      <option value="doubao">doubao</option>
                      <option value="anthropic">anthropic</option>
                    </select>
                  </label>
                </div>
                <div className="tc-settings-form__row">
                  <label className="tc-field">
                    <span>Provider</span>
                    <input
                      className="tc-input"
                      onChange={(event) =>
                        setForm((current) => ({ ...current, provider: event.target.value }))
                      }
                      placeholder="anthropic"
                      value={form.provider}
                    />
                  </label>
                  <label className="tc-field">
                    <span>API Key Env</span>
                    <input
                      className="tc-input"
                      onChange={(event) =>
                        setForm((current) => ({ ...current, apiKeyEnv: event.target.value }))
                      }
                      placeholder={inferApiKeyEnv(form.provider)}
                      value={form.apiKeyEnv ?? ""}
                    />
                  </label>
                </div>
                <label className="tc-field">
                  <span>Base URL</span>
                  <input
                    className="tc-input"
                    onChange={(event) =>
                      setForm((current) => ({ ...current, baseUrl: event.target.value }))
                    }
                    placeholder="https://api.example.com"
                    value={form.baseUrl ?? ""}
                  />
                </label>
                <div className="tc-settings-form__row">
                  <label className="tc-field">
                    <span>Context Window</span>
                    <input
                      className="tc-input"
                      inputMode="numeric"
                      onChange={(event) =>
                        setForm((current) => ({
                          ...current,
                          contextWindow: event.target.value
                            ? Number(event.target.value)
                            : null,
                        }))
                      }
                      value={form.contextWindow ?? ""}
                    />
                  </label>
                  <label className="tc-field">
                    <span>API Key</span>
                    <input
                      autoComplete="off"
                      className="tc-input"
                      onChange={(event) => setDraftApiKey(event.target.value)}
                      placeholder="Optional: save together"
                      type="password"
                      value={draftApiKey}
                    />
                  </label>
                </div>
                <div className="tc-settings-capabilities">
                  {(
                    [
                      ["vision", "Vision"],
                      ["files", "Files"],
                      ["tools", "Tools"],
                      ["reasoning", "Reasoning"],
                      ["webSearch", "Web Search"],
                    ] as const
                  ).map(([key, label]) => (
                    <label className="tc-settings-capabilities__item" key={key}>
                      <input
                        checked={form.capabilities[key]}
                        onChange={(event) =>
                          setForm((current) => ({
                            ...current,
                            capabilities: {
                              ...current.capabilities,
                              [key]: event.target.checked,
                            },
                          }))
                        }
                        type="checkbox"
                      />
                      <span>{label}</span>
                    </label>
                  ))}
                </div>
                <div className="tc-button-row tc-settings-form__actions">
                  <button
                    className="tc-button tc-button--ghost"
                    onClick={closeForm}
                    type="button"
                  >
                    Cancel
                  </button>
                  {selectedModel?.source === "user" ? (
                    <button
                      className="tc-button tc-button--ghost"
                      disabled={!state.capabilities.removeModel}
                      onClick={handleDelete}
                      type="button"
                    >
                      Delete
                    </button>
                  ) : null}
                  <button
                    className="tc-button tc-button--primary"
                    disabled={!state.capabilities.upsertModel || !form.id || !form.provider}
                    onClick={handleSave}
                    type="button"
                  >
                    Save Model
                  </button>
                </div>
              </div>
            </section>
          </div>
        ) : null}
      </main>
    </div>
  );
}
