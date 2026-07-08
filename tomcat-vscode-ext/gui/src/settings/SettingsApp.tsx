import { useEffect, useMemo, useState } from "react";

import type {
  SettingsHostFrame,
  SettingsIntent,
  SettingsModelCapabilities,
  SettingsModelInput,
  SettingsModelView,
  SettingsProviderKeyView,
  SettingsStateSnapshot,
  VsCodeApiLike,
} from "../../../src/shared/settingsProtocol";

type FormState = SettingsModelInput;

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

function modelRowMeta(model: SettingsModelView, providerKeys: SettingsProviderKeyView[]): string[] {
  const meta = [model.source, model.api];
  const key = providerKeys.find((entry) => entry.envName === model.apiKeyEnv);
  if (key?.provider) {
    meta.push(key.provider);
  } else if (model.provider) {
    meta.push(model.provider);
  }
  return meta;
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

  const loadModel = (model: SettingsModelView) => {
    setSelectedModelId(model.id);
    setDraftApiKey("");
    setValidationError(null);
    setForm(modelToForm(model));
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
      },
      type: "upsertModel",
    });
    if (draftApiKey.trim() && envName) {
      send(vscodeApi, {
        data: {
          envName,
          value: draftApiKey.trim(),
        },
        type: "setProviderKey",
      });
    }
    setDraftApiKey("");
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
    resetForm();
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
            <p>Manage built-in and custom models, then store API keys without echoing them back into the UI.</p>
          </div>
          <button className="tc-button tc-button--secondary" onClick={resetForm} type="button">
            New Model
          </button>
        </header>

        {state.error ? <div className="tc-banner tc-banner--warning">{state.error}</div> : null}
        {validationError ? <div className="tc-banner tc-banner--warning">{validationError}</div> : null}
        {state.status ? <div className="tc-banner">{state.status}</div> : null}

        {!state.capabilities.listModels ? (
          <section className="tc-empty-state">
            <h2>Model management unavailable</h2>
            <p>The connected `tomcat serve` does not expose the model management capabilities yet.</p>
          </section>
        ) : (
          <div className="tc-settings-grid">
            <section className="tc-card tc-settings-card">
              <div className="tc-card__header">
                <h3>{selectedModel ? `Edit ${selectedModel.id}` : "Add or Override Model"}</h3>
              </div>
              <div className="tc-settings-form">
                <label className="tc-field">
                  <span>Model ID</span>
                  <input
                    className="tc-input"
                    disabled={selectedModel !== null}
                    onChange={(event) => setForm((current) => ({ ...current, id: event.target.value }))}
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
                <div className="tc-button-row">
                  <button
                    className="tc-button tc-button--primary"
                    disabled={!state.capabilities.upsertModel || !form.id || !form.provider}
                    onClick={handleSave}
                    type="button"
                  >
                    Save Model
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
                </div>
              </div>
            </section>

            <section className="tc-card tc-settings-card">
              <div className="tc-card__header">
                <h3>Configured Models</h3>
              </div>
              <div className="tc-settings-groups">
                <div className="tc-settings-group">
                  <div className="tc-settings-group__title">Ready</div>
                  {readyModels.length === 0 ? (
                    <div className="tc-session-dropdown__empty">No ready models yet.</div>
                  ) : (
                    readyModels.map((model) => (
                      <article className="tc-settings-model" key={model.id}>
                        <div className="tc-settings-model__header">
                          <div>
                            <strong>{model.id}</strong>
                            <div className="tc-settings-model__meta">
                              {modelRowMeta(model, state.providerKeys).join(" · ")}
                            </div>
                          </div>
                          <div className="tc-button-row">
                            <span className="tc-chip tc-chip--success">Ready</span>
                            <button
                              className="tc-button tc-button--secondary"
                              onClick={() => loadModel(model)}
                              type="button"
                            >
                              Edit
                            </button>
                          </div>
                        </div>
                        <div className="tc-settings-model__footer">{model.apiKeyEnv}</div>
                      </article>
                    ))
                  )}
                </div>

                <div className="tc-settings-group">
                  <div className="tc-settings-group__title">Needs API Key</div>
                  {needsKeyModels.length === 0 ? (
                    <div className="tc-session-dropdown__empty">All visible models are ready.</div>
                  ) : (
                    needsKeyModels.map((model) => (
                      <article className="tc-settings-model" key={model.id}>
                        <div className="tc-settings-model__header">
                          <div>
                            <strong>{model.id}</strong>
                            <div className="tc-settings-model__meta">
                              {modelRowMeta(model, state.providerKeys).join(" · ")}
                            </div>
                          </div>
                          <button
                            className="tc-button tc-button--secondary"
                            onClick={() => loadModel(model)}
                            type="button"
                          >
                            Edit
                          </button>
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
                            placeholder={`Save ${model.apiKeyEnv}`}
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
                    ))
                  )}
                </div>
              </div>
            </section>
          </div>
        )}
      </main>
    </div>
  );
}
