import { useEffect, useMemo, useRef, useState } from "react";

import type { MessageKey, Translator } from "../../../src/shared/i18n";
import type {
  SettingsIntent,
  SettingsModelCapabilities,
  SettingsModelInput,
  SettingsModelView,
  SettingsProviderKeyView,
  SettingsStateSnapshot,
  VsCodeApiLike,
} from "../../../src/shared/settingsProtocol";
import { useT } from "../i18n/LocaleProvider";
import { parseCommaList } from "./commaList";
import { KeySlotCombobox, type KeySlotOption } from "./KeySlotCombobox";
import { isValidKeySlotName } from "./keySlot";
import { findMatchingProviderPreset, PROVIDER_PRESETS } from "./providerPresets";
import {
  API_OPTIONS,
  baseUrlProblem,
  defaultThinkingFormatForApi,
  fieldText,
  normalizeBaseUrlInput,
  send,
  THINKING_FORMAT_OPTIONS,
} from "./settingsShared";

const CAPABILITY_OPTIONS = [
  ["vision", "models.capability.vision"],
  ["files", "models.capability.files"],
  ["tools", "models.capability.tools"],
  ["reasoning", "models.capability.reasoning"],
  ["webSearch", "models.capability.webSearch"],
] as const;

const PRESET_URL_LIST_ID = "settings-model-preset-urls";

interface FormState {
  api: string;
  apiKeyEnv: string;
  baseUrl: string;
  capabilities: SettingsModelCapabilities;
  contextWindow: string;
  description: string;
  maxOutputTokens: string;
  modelName: string;
  provider: string;
  reasoningLevels: string;
  supportsFast: boolean;
  thinkingFormat: string;
}

function modelToForm(model: SettingsModelView): FormState {
  return {
    api: model.api,
    apiKeyEnv: model.apiKeyEnv,
    baseUrl: model.baseUrl ?? "",
    capabilities: { ...model.capabilities },
    contextWindow: typeof model.contextWindow === "number" ? String(model.contextWindow) : "",
    description: model.description ?? "",
    maxOutputTokens: typeof model.maxOutputTokens === "number" ? String(model.maxOutputTokens) : "",
    modelName: fieldText(model.modelName) || model.id,
    provider: model.provider,
    reasoningLevels: model.supportedReasoningLevels?.join(", ") ?? "",
    supportsFast: model.supportedSpeeds?.includes("fast") ?? false,
    thinkingFormat: model.thinkingFormat || defaultThinkingFormatForApi(model.api),
  };
}

/** Empty means "runtime default"; anything else must be a positive integer. */
function parseTokenCount(value: string): number | null | undefined {
  const trimmed = value.trim();
  if (!trimmed) return null;
  return /^\d+$/.test(trimmed) && Number(trimmed) > 0 ? Number(trimmed) : undefined;
}

function buildKeySlotOptions(current: string, providerKeys: SettingsProviderKeyView[], t: Translator): KeySlotOption[] {
  const options: KeySlotOption[] = [];
  const known = providerKeys.find((entry) => entry.envName === current);
  if (current) {
    options.push({
      envName: current,
      group: "suggested",
      keyPresent: known?.keyPresent ?? false,
      label: t(known?.keyPresent ? "models.keyConfigured" : "models.keySuggested", { name: current }),
    });
  }
  for (const entry of providerKeys) {
    if (entry.envName === current) continue;
    options.push({
      envName: entry.envName,
      group: "saved",
      keyPresent: entry.keyPresent,
      label: entry.keyPresent ? t("models.keyConfigured", { name: entry.envName }) : entry.envName,
    });
  }
  return options;
}

interface ModelEditDialogProps {
  model: SettingsModelView;
  onClose(): void;
  onDelete(modelId: string): void;
  state: SettingsStateSnapshot;
  vscodeApi: VsCodeApiLike<SettingsIntent>;
}

export function ModelEditDialog({ model, onClose, onDelete, state, vscodeApi }: ModelEditDialogProps) {
  const t = useT();
  const [form, setForm] = useState<FormState>(() => modelToForm(model));
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [problem, setProblem] = useState<MessageKey | null>(null);
  const [lastSaveId, setLastSaveId] = useState<string | null>(null);
  const [isKeySlotRefreshing, setIsKeySlotRefreshing] = useState(false);
  const [keysRefreshed, setKeysRefreshed] = useState(false);
  const providerKeysRef = useRef(state.providerKeys);

  const receipt = state.modelSaveReceipt;
  const saving = lastSaveId !== null && receipt?.requestId !== lastSaveId;
  const saveFinished = lastSaveId !== null && receipt?.requestId === lastSaveId;

  useEffect(() => {
    if (saveFinished && receipt?.success) onClose();
  }, [onClose, receipt?.success, saveFinished]);

  useEffect(() => {
    if (providerKeysRef.current === state.providerKeys) return;
    providerKeysRef.current = state.providerKeys;
    if (isKeySlotRefreshing) {
      setIsKeySlotRefreshing(false);
      setKeysRefreshed(true);
    }
  }, [isKeySlotRefreshing, state.providerKeys]);

  useEffect(() => {
    if (!keysRefreshed) return;
    const timer = window.setTimeout(() => setKeysRefreshed(false), 2500);
    return () => window.clearTimeout(timer);
  }, [keysRefreshed]);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !saving) {
        event.preventDefault();
        onClose();
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose, saving]);

  const matchedPreset = useMemo(
    () => findMatchingProviderPreset({ api: form.api, baseUrl: normalizeBaseUrlInput(form.baseUrl), provider: form.provider }),
    [form.api, form.baseUrl, form.provider],
  );
  const keySlotOptions = useMemo(
    () => buildKeySlotOptions(fieldText(form.apiKeyEnv), state.providerKeys, t),
    [form.apiKeyEnv, state.providerKeys, t],
  );
  const name = fieldText(form.modelName);
  const unprobed = model.source === "user"
    && !state.models.some((entry) => entry.source === "builtin" && (fieldText(entry.modelName) || entry.id) === name);
  const tiers = model.contextWindowOptions ?? [];

  function update<K extends keyof FormState>(key: K, value: FormState[K]): void {
    setForm((current) => ({ ...current, [key]: value }));
  }

  function handleApiChange(nextApi: string): void {
    setForm((current) => ({
      ...current,
      api: nextApi,
      thinkingFormat: current.thinkingFormat === defaultThinkingFormatForApi(current.api)
        ? defaultThinkingFormatForApi(nextApi)
        : current.thinkingFormat,
    }));
  }

  function buildInput(): SettingsModelInput | MessageKey {
    const urlProblem = baseUrlProblem(form.baseUrl);
    if (urlProblem) return urlProblem;
    if (!name) return "models.validate.name";
    const apiKeyEnv = fieldText(form.apiKeyEnv);
    if (!apiKeyEnv) return "models.validate.slotRequired";
    if (!isValidKeySlotName(apiKeyEnv)) return "models.validate.slotInvalid";
    if (!fieldText(form.provider) || !fieldText(form.api)) return "models.validate.identity";
    const contextWindow = parseTokenCount(form.contextWindow);
    const maxOutputTokens = parseTokenCount(form.maxOutputTokens);
    if (contextWindow === undefined || maxOutputTokens === undefined
      || (contextWindow !== null && maxOutputTokens !== null && maxOutputTokens > contextWindow)) {
      return "models.validate.tokens";
    }
    const reasoningLevels = parseCommaList(form.reasoningLevels);
    return {
      api: fieldText(form.api),
      apiKeyEnv,
      baseUrl: normalizeBaseUrlInput(form.baseUrl),
      capabilities: { ...form.capabilities },
      contextWindow,
      contextWindowOptions: tiers.length > 0 ? [...tiers] : null,
      description: fieldText(form.description) || null,
      id: model.id,
      maxOutputTokens,
      modelName: !model.modelName && name === model.id ? null : name,
      provider: fieldText(form.provider),
      supportedReasoningLevels: reasoningLevels.length > 0 ? reasoningLevels : null,
      supportedSpeeds: form.supportsFast ? ["fast"] : null,
      thinkingFormat: fieldText(form.thinkingFormat) || null,
    };
  }

  function handleSave(): void {
    if (saving) return;
    const input = buildInput();
    if (typeof input === "string") {
      setProblem(input);
      return;
    }
    setProblem(null);
    setLastSaveId(send(vscodeApi, { data: { model: input }, type: "upsertModel" }));
  }

  function handleKeySlotRefresh(): void {
    if (!state.capabilities.listProviderKeys || isKeySlotRefreshing) return;
    setIsKeySlotRefreshing(true);
    send(vscodeApi, { type: "listProviderKeys" });
  }

  return (
    <div className="tc-settings-modal" onClick={saving ? undefined : onClose}>
      <section
        aria-labelledby="settings-model-form-title"
        aria-modal="true"
        className="tc-card tc-settings-modal__card"
        data-testid="settings-model-form"
        onClick={(event) => event.stopPropagation()}
        role="dialog"
      >
        <div className="tc-settings-modal__header">
          <div>
            <h3 id="settings-model-form-title">{t("models.editTitle", { name: model.id })}</h3>
            <p>{t("models.editDescription")}</p>
          </div>
          <button
            aria-label={t("models.closeForm")}
            className="tc-icon-button tc-settings-modal__close"
            data-testid="settings-close-model-form"
            disabled={saving}
            onClick={onClose}
            type="button"
          >
            <span aria-hidden="true" className="codicon codicon-close" />
          </button>
        </div>

        {problem ? <div className="tc-banner tc-banner--warning" role="alert">{t(problem)}</div> : null}

        <fieldset className="tc-settings-form" disabled={saving}>
          <label className="tc-field">
            <span>{t("models.editAddress")} *</span>
            <input
              className="tc-input"
              data-testid="settings-model-base-url"
              list={PRESET_URL_LIST_ID}
              onChange={(event) => update("baseUrl", event.target.value)}
              placeholder="https://api.example.com/v1"
              value={form.baseUrl}
            />
            <datalist id={PRESET_URL_LIST_ID}>
              {PROVIDER_PRESETS.map((preset) => (
                <option key={preset.provider} value={preset.baseUrl}>{t(preset.labelKey)}</option>
              ))}
            </datalist>
            {matchedPreset ? (
              <small className="tc-field__hint" data-testid="settings-model-address-preset">
                {t("models.addressPreset", { name: t(matchedPreset.labelKey) })}
              </small>
            ) : null}
          </label>
          <label className="tc-field">
            <span>{t("models.name")} *</span>
            <input
              className="tc-input"
              data-testid="settings-model-name"
              onChange={(event) => update("modelName", event.target.value)}
              placeholder={t("models.example", { name: "gpt-5.6" })}
              value={form.modelName}
            />
            <small className="tc-field__hint">{t("models.exactName")}</small>
          </label>

          <button
            aria-expanded={showAdvanced}
            className="tc-settings-advanced__toggle"
            data-testid="settings-model-advanced"
            onClick={() => setShowAdvanced((current) => !current)}
            type="button"
          >
            <span>{t("models.advanced")}</span>
            <span className="tc-settings-advanced__caret">{showAdvanced ? "▾" : "▸"}</span>
          </button>

          {showAdvanced ? (
            <div className="tc-settings-advanced">
              <div className="tc-settings-form__row">
                <label className="tc-field">
                  <span>{t("models.protocol")}</span>
                  <select
                    aria-label={t("models.protocol")}
                    data-testid="settings-model-api"
                    onChange={(event) => handleApiChange(event.target.value)}
                    value={form.api}
                  >
                    {API_OPTIONS.map((entry) => (
                      <option key={entry.value} value={entry.value}>{t(entry.labelKey)}</option>
                    ))}
                  </select>
                  <small className="tc-field__hint">{t("models.apiHint")}</small>
                </label>
                <label className="tc-field">
                  <span>{t("models.providerOverride")}</span>
                  <input
                    className="tc-input"
                    onChange={(event) => update("provider", event.target.value)}
                    value={form.provider}
                  />
                  <small className="tc-field__hint">{t("models.providerHint")}</small>
                </label>
              </div>

              <div className="tc-settings-form__row" data-testid="settings-key-fields-row">
                <KeySlotCombobox
                  feedback={keysRefreshed ? t("models.keysRefreshed") : null}
                  hint={t("models.keySearchHint")}
                  onChange={(envName) => update("apiKeyEnv", envName)}
                  onRefresh={handleKeySlotRefresh}
                  options={keySlotOptions}
                  placeholder="EXAMPLE_API_KEY"
                  refreshDisabled={!state.capabilities.listProviderKeys || isKeySlotRefreshing}
                  refreshLabel={t("models.keyRefresh")}
                  refreshing={isKeySlotRefreshing}
                  value={form.apiKeyEnv}
                />
                <label className="tc-field">
                  <span>{t("models.alias")}</span>
                  <input
                    aria-label={t("models.alias")}
                    className="tc-input tc-input--readonly tc-settings-keyslot-peer"
                    data-testid="settings-model-alias"
                    readOnly
                    value={model.id}
                  />
                  <small className="tc-field__hint">{t("models.aliasFixed")}</small>
                </label>
              </div>

              <div className="tc-settings-form__row">
                <label className="tc-field">
                  <span>{t("models.thinkingFormat")}</span>
                  <select
                    aria-label={t("models.thinkingFormat")}
                    onChange={(event) => update("thinkingFormat", event.target.value)}
                    value={form.thinkingFormat}
                  >
                    {THINKING_FORMAT_OPTIONS.map((entry) => (
                      <option key={entry.value} value={entry.value}>{t(entry.labelKey)}</option>
                    ))}
                  </select>
                  <small className="tc-field__hint">{t("models.thinkingHint")}</small>
                </label>
                <label className="tc-field">
                  <span>{t("models.effortLevels")}</span>
                  <input
                    aria-label={t("models.effortLevels")}
                    className="tc-input"
                    data-testid="settings-supported-effort-levels"
                    onChange={(event) => update("reasoningLevels", event.target.value)}
                    placeholder="low, medium, high, xhigh, max"
                    value={form.reasoningLevels}
                  />
                  <small className="tc-field__hint">{t("models.effortHint")}</small>
                </label>
              </div>

              <div className="tc-settings-form__row">
                <label className="tc-field">
                  <span>{t("models.contextWindow")}</span>
                  {tiers.length > 0 ? (
                    <select
                      aria-label={t("models.contextWindow")}
                      data-testid="settings-context-window"
                      onChange={(event) => update("contextWindow", event.target.value)}
                      value={form.contextWindow}
                    >
                      {tiers.map((tier) => <option key={tier} value={String(tier)}>{tier}</option>)}
                    </select>
                  ) : (
                    <input
                      aria-label={t("models.contextWindow")}
                      className="tc-input"
                      data-testid="settings-context-window"
                      inputMode="numeric"
                      onChange={(event) => update("contextWindow", event.target.value)}
                      placeholder={t("models.contextAuto")}
                      value={form.contextWindow}
                    />
                  )}
                  <small className="tc-field__hint">{t("models.contextHint")}</small>
                </label>
                <label className="tc-field">
                  <span>{t("models.maxOutput")}</span>
                  <input
                    aria-label={t("models.maxOutput")}
                    className="tc-input"
                    data-testid="settings-max-output-tokens"
                    inputMode="numeric"
                    onChange={(event) => update("maxOutputTokens", event.target.value)}
                    placeholder={t("models.contextAuto")}
                    value={form.maxOutputTokens}
                  />
                  <small className="tc-field__hint">{t("models.maxOutputHint")}</small>
                </label>
              </div>

              <label className="tc-settings-capabilities__item">
                <input
                  checked={form.supportsFast}
                  data-testid="settings-supported-speeds"
                  onChange={(event) => update("supportsFast", event.target.checked)}
                  type="checkbox"
                />
                <span>{t("models.speeds")}</span>
              </label>
              <small className="tc-field__hint">{t("models.speedsHint")}</small>

              <div className="tc-settings-capabilities">
                {CAPABILITY_OPTIONS.map(([key, label]) => (
                  <label className="tc-settings-capabilities__item" key={key}>
                    <input
                      checked={form.capabilities[key]}
                      onChange={(event) => update("capabilities", { ...form.capabilities, [key]: event.target.checked })}
                      type="checkbox"
                    />
                    <span>{t(label)}</span>
                  </label>
                ))}
              </div>
              {unprobed ? (
                <small className="tc-field__hint" data-testid="settings-capability-unprobed">
                  {t("models.capabilityUnprobed")}
                </small>
              ) : null}

              <label className="tc-field">
                <span>{t("models.descriptionField")}</span>
                <input
                  className="tc-input"
                  onChange={(event) => update("description", event.target.value)}
                  placeholder={t("models.descriptionOptional")}
                  value={form.description}
                />
              </label>
            </div>
          ) : null}

          {saveFinished && !receipt?.success ? (
            <div role="status">
              {state.error ? <p className="tc-banner tc-banner--warning">{state.error}</p> : null}
              {state.warnings?.map((warning) => <p key={warning}>{warning}</p>)}
            </div>
          ) : null}

          <div className="tc-button-row tc-settings-form__actions">
            <button className="tc-button tc-button--ghost" onClick={onClose} type="button">
              {t("common.cancel")}
            </button>
            {model.source === "user" ? (
              <button
                className="tc-button tc-button--ghost"
                disabled={!state.capabilities.removeModel}
                onClick={() => onDelete(model.id)}
                type="button"
              >
                {t("models.delete")}
              </button>
            ) : null}
            <button
              className="tc-button tc-button--primary"
              data-testid="settings-save-model"
              disabled={!state.capabilities.upsertModel}
              onClick={handleSave}
              title={state.capabilities.upsertModel ? undefined : t("models.disable.write")}
              type="button"
            >
              {t("models.saveModel")}
            </button>
          </div>
        </fieldset>
      </section>
    </div>
  );
}
