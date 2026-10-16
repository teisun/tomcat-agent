import { useEffect, useMemo, useState } from "react";

import type { MessageKey } from "../../../src/shared/i18n";
import type {
  SettingsIntent,
  SettingsModelCapabilities,
  SettingsModelInput,
  SettingsModelView,
  SettingsStateSnapshot,
  VsCodeApiLike,
} from "../../../src/shared/settingsProtocol";
import { useT } from "../i18n/LocaleProvider";
import { defaultProtocolFor, defaultSupportsFast, findReusableModelByName } from "./modelBuiltinMatch";
import { buildProviderCards, keyStatusLabel, type ProviderCard } from "./providerCards";
import { PROVIDER_PRESETS, type ProviderPreset } from "./providerPresets";
import {
  deriveRelayFields,
  envNameForRelaySlug,
  findConfiguredRelayByBaseUrl,
  RELAY_ID_SEPARATOR,
} from "./relayDerive";
import {
  ALL_PROTOCOLS,
  API_OPTIONS,
  baseUrlProblem,
  defaultThinkingFormatForApi,
  fieldText,
  normalizeBaseUrlInput,
  send,
} from "./settingsShared";

export const OTHER_ADDRESS = "other";

export type ConnectTarget =
  | { kind: "card"; keyEnv: string }
  | { kind: "new"; preset: string };

/** Unknown models are declared optimistically; the edit form marks them as unprobed. */
const OPTIMISTIC_CAPABILITIES: SettingsModelCapabilities = {
  files: true,
  reasoning: true,
  tools: true,
  vision: true,
  webSearch: false,
};

const NEW_KEY = "\u0000new";

type Problem = { args?: Record<string, string | number>; key: MessageKey };

interface ModelRow {
  /** Set when the row is already in the target card or its ID already exists. */
  existingId: string | null;
  inTargetSlot: boolean;
  locked: "existing" | "included" | null;
  name: string;
}

interface ConnectProviderDialogProps {
  cards: readonly ProviderCard[];
  onClose(): void;
  presetLabel(preset: ProviderPreset): string;
  state: SettingsStateSnapshot;
  target: ConnectTarget;
  vscodeApi: VsCodeApiLike<SettingsIntent>;
}

function modelNameOf(model: SettingsModelView): string {
  return fieldText(model.modelName) || model.id;
}

function copyList<T>(values: readonly T[] | null | undefined): T[] | null {
  return values && values.length > 0 ? [...values] : null;
}

export function ConnectProviderDialog({ cards, onClose, presetLabel, state, target, vscodeApi }: ConnectProviderDialogProps) {
  const t = useT();
  const card = target.kind === "card" ? cards.find((entry) => entry.keyEnv === target.keyEnv) ?? null : null;
  const [presetChoice, setPresetChoice] = useState(target.kind === "new" ? target.preset : OTHER_ADDRESS);
  const [address, setAddress] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [keyChoice, setKeyChoice] = useState<string | null>(null);
  const [remoteRequestId, setRemoteRequestId] = useState<string | null>(null);
  const [manualNames, setManualNames] = useState<string[]>([]);
  const [manualDraft, setManualDraft] = useState("");
  const [search, setSearch] = useState("");
  const [checked, setChecked] = useState<ReadonlySet<string>>(() => new Set());
  const [protocols, setProtocols] = useState<Readonly<Record<string, string>>>({});
  const [setAsDefault, setSetAsDefault] = useState(true);
  const [problem, setProblem] = useState<Problem | null>(null);
  const [lastSaveId, setLastSaveId] = useState<string | null>(null);

  const receipt = state.modelSaveReceipt;
  const saving = lastSaveId !== null && receipt?.requestId !== lastSaveId;
  const saveFinished = lastSaveId !== null && receipt?.requestId === lastSaveId;

  useEffect(() => {
    if (saveFinished && receipt?.success) onClose();
  }, [onClose, receipt?.success, saveFinished]);

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

  const preset = card ? card.preset : PROVIDER_PRESETS.find((entry) => entry.provider === presetChoice) ?? null;
  const isOther = !card && !preset;
  const addressProblem = isOther ? baseUrlProblem(address) : null;
  const baseUrl = card
    ? card.preset?.baseUrl ?? card.anchor.baseUrl ?? ""
    : preset ? preset.baseUrl : normalizeBaseUrlInput(address) ?? "";
  const candidates = preset ? [preset.api] : ALL_PROTOCOLS;
  const derivedSlug = useMemo(() => deriveRelayFields(baseUrl, "").slug, [baseUrl]);
  const slug = preset?.provider ?? derivedSlug;
  const provider = card?.anchor.provider ?? preset?.provider ?? derivedSlug;

  const sameAddressCards = useMemo(() => {
    if (card || addressProblem || !baseUrl) return [];
    const slots = new Set(findConfiguredRelayByBaseUrl(state.models, baseUrl));
    if (preset) slots.add(preset.apiKeyEnv);
    return cards.filter((entry) => entry.keyPresent && slots.has(entry.keyEnv));
  }, [addressProblem, baseUrl, card, cards, preset, state.models]);
  const chosenCard = keyChoice === NEW_KEY
    ? null
    : sameAddressCards.find((entry) => entry.keyEnv === keyChoice) ?? sameAddressCards[0] ?? null;

  const newSlot = useMemo(() => {
    if (preset && !cards.some((entry) => entry.keyEnv === preset.apiKeyEnv && entry.keyPresent)) {
      return preset.apiKeyEnv;
    }
    const occupied = new Set([
      ...state.models.map((model) => model.apiKeyEnv),
      ...state.providerKeys.map((entry) => entry.envName),
      ...PROVIDER_PRESETS.map((entry) => entry.apiKeyEnv),
    ]);
    return envNameForRelaySlug(slug, occupied);
  }, [cards, preset, slug, state.models, state.providerKeys]);
  const targetSlot = card?.keyEnv ?? chosenCard?.keyEnv ?? newSlot;
  const targetKeyPresent = card ? card.keyPresent : chosenCard !== null;
  const needsKeyInput = !card && !chosenCard;

  const newCardTitle = useMemo(() => {
    if (sameAddressCards.length === 0) return "";
    const placeholder: SettingsModelView = {
      api: candidates[0],
      apiKeyEnv: newSlot,
      baseUrl,
      capabilities: OPTIMISTIC_CAPABILITIES,
      id: `${slug}${RELAY_ID_SEPARATOR}\u0000`,
      keyPresent: true,
      provider,
      source: "user",
    };
    return buildProviderCards({ models: [...state.models, placeholder], presetLabel })
      .cards.find((entry) => entry.keyEnv === newSlot)?.title ?? "";
  }, [baseUrl, candidates, newSlot, presetLabel, provider, sameAddressCards.length, slug, state.models]);

  const remoteReceipt = remoteRequestId !== null && state.remoteModelsReceipt?.requestId === remoteRequestId
    ? state.remoteModelsReceipt
    : null;
  const fetching = remoteRequestId !== null && remoteReceipt === null;
  const remoteNames = useMemo(() => remoteReceipt?.models ?? [], [remoteReceipt]);

  const rows = useMemo<ModelRow[]>(() => {
    const result: ModelRow[] = [];
    const seen = new Set<string>();
    const push = (row: ModelRow) => {
      if (row.name && !seen.has(row.name)) {
        seen.add(row.name);
        result.push(row);
      }
    };
    for (const model of state.models) {
      if (model.apiKeyEnv !== targetSlot) continue;
      push({
        existingId: model.id,
        inTargetSlot: true,
        locked: !card && !targetKeyPresent && model.source === "builtin" ? "included" : "existing",
        name: modelNameOf(model),
      });
    }
    for (const name of [...remoteNames, ...manualNames]) {
      const clash = state.models.find((model) => model.id === `${slug}${RELAY_ID_SEPARATOR}${name}`);
      push({ existingId: clash?.id ?? null, inTargetSlot: false, locked: clash ? "existing" : null, name });
    }
    return result;
  }, [card, manualNames, remoteNames, slug, state.models, targetKeyPresent, targetSlot]);

  const needle = search.trim().toLowerCase();
  const visibleRows = needle ? rows.filter((row) => row.name.toLowerCase().includes(needle)) : rows;
  const selectedNames = rows.filter((row) => !row.locked && checked.has(row.name)).map((row) => row.name);
  const firstId = rows.find((row) => row.inTargetSlot)?.existingId
    ?? (selectedNames[0] ? `${slug}${RELAY_ID_SEPARATOR}${selectedNames[0]}` : null);
  const credentialsAfterSave = targetKeyPresent || (needsKeyInput && apiKey.trim() !== "");
  const defaultReady = Boolean(state.defaultModelId)
    && state.models.some((model) => model.id === state.defaultModelId && model.keyPresent);
  const showDefaultHint = state.defaultModelId === ""
    && state.capabilities.setDefaultModelIfUnset === true
    && credentialsAfterSave
    && firstId !== null;
  const showSetAsDefault = Boolean(state.defaultModelId)
    && !defaultReady
    && state.capabilities.setDefaultModel === true
    && firstId !== null;

  function protocolFor(name: string): string {
    const chosen = protocols[name];
    return chosen && candidates.includes(chosen)
      ? chosen
      : defaultProtocolFor(name, findReusableModelByName(state.models, name), candidates);
  }

  function buildEntry(name: string): SettingsModelInput {
    const reference = findReusableModelByName(state.models, name);
    const api = protocolFor(name);
    return {
      api,
      apiKeyEnv: targetSlot,
      baseUrl,
      capabilities: { ...(reference?.capabilities ?? OPTIMISTIC_CAPABILITIES) },
      contextWindow: reference?.contextWindow ?? null,
      contextWindowOptions: copyList(reference?.contextWindowOptions),
      description: null,
      id: `${slug}${RELAY_ID_SEPARATOR}${name}`,
      maxOutputTokens: reference?.maxOutputTokens ?? null,
      modelName: name,
      provider,
      supportedReasoningLevels: copyList(reference?.supportedReasoningLevels),
      supportedSpeeds: defaultSupportsFast(name, reference) ? ["fast"] : null,
      thinkingFormat: (reference?.api === api ? reference.thinkingFormat : null)
        ?? preset?.thinkingFormat
        ?? defaultThinkingFormatForApi(api),
    };
  }

  function resetConnection(): void {
    setRemoteRequestId(null);
    setKeyChoice(null);
    setProblem(null);
  }

  function toggle(name: string, on: boolean): void {
    setChecked((current) => {
      const next = new Set(current);
      if (on) next.add(name);
      else next.delete(name);
      return next;
    });
  }

  function addManual(): void {
    const name = manualDraft.trim();
    if (!name) return;
    if (rows.some((row) => row.name === name)) {
      setProblem({ args: { name }, key: "models.connect.validate.duplicate" });
      return;
    }
    setProblem(null);
    setManualNames((current) => [...current, name]);
    toggle(name, true);
    setManualDraft("");
  }

  function fetchRemote(): void {
    if (addressProblem) {
      setProblem({ key: addressProblem });
      return;
    }
    setProblem(null);
    const typedKey = needsKeyInput ? apiKey.trim() : "";
    const credential = typedKey
      ? { apiKey: typedKey }
      : targetKeyPresent ? { apiKeyEnv: targetSlot } : {};
    setRemoteRequestId(send(vscodeApi, {
      data: { api: preset?.api ?? "openai", baseUrl, ...credential },
      type: "listRemoteModels",
    }));
  }

  function handleSave(): void {
    if (saving) return;
    if (addressProblem) {
      setProblem({ key: addressProblem });
      return;
    }
    if (needsKeyInput && !apiKey.trim()) {
      setProblem({ key: "models.connect.validate.keyRequired" });
      return;
    }
    const includesProvider = needsKeyInput && rows.some((row) => row.locked === "included");
    if (selectedNames.length === 0 && !includesProvider) {
      setProblem({ key: "models.connect.validate.noModels" });
      return;
    }
    setProblem(null);
    const explicitDefault = showSetAsDefault && setAsDefault;
    setLastSaveId(send(vscodeApi, {
      data: {
        models: selectedNames.map(buildEntry),
        ...(needsKeyInput ? { providerKey: { envName: targetSlot, value: apiKey.trim() } } : {}),
        ...((showDefaultHint || explicitDefault) && firstId ? { defaultModelId: firstId } : {}),
        ...(explicitDefault ? { setAsDefault: true } : {}),
      },
      type: "upsertModels",
    }));
  }

  const title = card ? t("models.connect.addTitle", { name: card.title }) : t("models.connect.title");
  const showProtocolPicker = candidates.length > 1;
  return (
    <div className="tc-settings-modal" onClick={saving ? undefined : onClose}>
      <section
        aria-labelledby="settings-connect-title"
        aria-modal="true"
        className="tc-card tc-settings-modal__card tc-settings-connect"
        data-testid="settings-connect-dialog"
        onClick={(event) => event.stopPropagation()}
        role="dialog"
      >
        <div className="tc-settings-modal__header">
          <div>
            <h3 id="settings-connect-title">{title}</h3>
            {card ? <p>{[...card.hosts, keyStatusLabel(card, t)].join(" · ")}</p> : null}
          </div>
          <button
            aria-label={t("common.close")}
            className="tc-icon-button tc-settings-modal__close"
            data-testid="settings-connect-close"
            disabled={saving}
            onClick={onClose}
            type="button"
          >
            <span aria-hidden="true" className="codicon codicon-close" />
          </button>
        </div>

        {problem ? <div className="tc-banner tc-banner--warning" role="alert">{t(problem.key, problem.args)}</div> : null}

        <fieldset className="tc-settings-form" disabled={saving}>
          {card ? null : (
            <>
              <label className="tc-field">
                <span>{t("models.connect.provider")}</span>
                <select
                  data-testid="settings-connect-preset"
                  onChange={(event) => {
                    setPresetChoice(event.target.value);
                    setManualNames([]);
                    setChecked(new Set());
                    setProtocols({});
                    resetConnection();
                  }}
                  value={presetChoice}
                >
                  {PROVIDER_PRESETS.map((entry) => (
                    <option key={entry.provider} value={entry.provider}>{presetLabel(entry)}</option>
                  ))}
                  <option value={OTHER_ADDRESS}>{t("models.preset.other")}</option>
                </select>
              </label>
              <label className="tc-field">
                <span>{t("models.connect.address")} *</span>
                <input
                  className={`tc-input${isOther ? "" : " tc-input--readonly"}`}
                  data-testid="settings-connect-address"
                  onChange={(event) => {
                    setAddress(event.target.value);
                    resetConnection();
                  }}
                  placeholder="https://api.example.com/v1"
                  readOnly={!isOther}
                  value={isOther ? address : preset?.baseUrl ?? ""}
                />
              </label>
            </>
          )}

          {sameAddressCards.length > 0 ? (
            <fieldset className="tc-settings-connect__same">
              <legend>{t("models.connect.sameAddress")}</legend>
              {sameAddressCards.map((entry) => (
                <label className="tc-settings-capabilities__item" key={entry.keyEnv}>
                  <input
                    checked={chosenCard?.keyEnv === entry.keyEnv}
                    data-testid={`settings-connect-same-${entry.keyEnv}`}
                    name="settings-connect-same"
                    onChange={() => {
                      setKeyChoice(entry.keyEnv);
                      setRemoteRequestId(null);
                    }}
                    type="radio"
                  />
                  <span>{t("models.connect.useSavedKey", { name: entry.title })}</span>
                </label>
              ))}
              <label className="tc-settings-capabilities__item">
                <input
                  checked={chosenCard === null}
                  data-testid="settings-connect-same-new"
                  name="settings-connect-same"
                  onChange={() => {
                    setKeyChoice(NEW_KEY);
                    setRemoteRequestId(null);
                  }}
                  type="radio"
                />
                <span>{t("models.connect.useNewKey", { name: newCardTitle })}</span>
              </label>
            </fieldset>
          ) : null}

          {needsKeyInput ? (
            <label className="tc-field">
              <span>{t("term.field.apiKey")} *</span>
              <input
                autoComplete="off"
                className="tc-input"
                data-testid="settings-connect-api-key"
                onChange={(event) => setApiKey(event.target.value)}
                placeholder={t("models.card.keyPlaceholder")}
                type="password"
                value={apiKey}
              />
            </label>
          ) : null}

          <div className="tc-settings-connect__models">
            <div className="tc-settings-connect__models-header">
              <span>{t("models.connect.models")}</span>
              <div className="tc-settings-connect__tools">
                <button
                  className="tc-button tc-button--secondary"
                  data-testid="settings-connect-fetch"
                  disabled={fetching}
                  onClick={fetchRemote}
                  type="button"
                >
                  {fetching ? t("models.connect.fetching") : t("models.connect.fetch")}
                </button>
                <input
                  aria-label={t("models.connect.search")}
                  className="tc-input"
                  data-testid="settings-connect-search"
                  onChange={(event) => setSearch(event.target.value)}
                  placeholder={t("models.connect.search")}
                  value={search}
                />
              </div>
            </div>
            {remoteReceipt?.failure ? (
              <div className="tc-banner tc-banner--warning" data-testid="settings-connect-fetch-failed" role="status">
                {t(`models.connect.failed.${remoteReceipt.failure}`)}
              </div>
            ) : remoteReceipt && remoteNames.length === 0 ? (
              <div className="tc-banner" role="status">{t("models.connect.fetchedNone")}</div>
            ) : null}
            <ul className="tc-settings-connect__list">
              {visibleRows.map((row) => {
                const isChecked = row.locked !== null || checked.has(row.name);
                return (
                  <li className="tc-settings-connect__row" key={row.name}>
                    <label
                      className="tc-settings-capabilities__item"
                      title={row.locked === "included"
                        ? t("models.connect.included")
                        : row.locked === "existing" ? t("models.connect.existing") : undefined}
                    >
                      <input
                        checked={isChecked}
                        data-testid={`settings-connect-model-${row.name}`}
                        disabled={row.locked !== null}
                        onChange={(event) => toggle(row.name, event.target.checked)}
                        type="checkbox"
                      />
                      <span>{row.name}</span>
                    </label>
                    {showProtocolPicker && !row.locked && isChecked ? (
                      <select
                        aria-label={t("models.connect.protocolFor", { name: row.name })}
                        className="tc-input"
                        data-testid={`settings-connect-protocol-${row.name}`}
                        onChange={(event) => setProtocols((current) => ({ ...current, [row.name]: event.target.value }))}
                        value={protocolFor(row.name)}
                      >
                        {API_OPTIONS.map((option) => (
                          <option key={option.value} value={option.value}>{t(option.labelKey)}</option>
                        ))}
                      </select>
                    ) : null}
                  </li>
                );
              })}
            </ul>
            <div className="tc-settings-connect__manual">
              <input
                aria-label={t("models.connect.manualPlaceholder")}
                className="tc-input"
                data-testid="settings-connect-manual-input"
                onChange={(event) => setManualDraft(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    addManual();
                  }
                }}
                placeholder={t("models.connect.manualPlaceholder")}
                value={manualDraft}
              />
              <button
                className="tc-button tc-button--secondary"
                data-testid="settings-connect-manual-add"
                onClick={addManual}
                type="button"
              >
                {t("models.connect.manualAdd")}
              </button>
            </div>
          </div>

          {showDefaultHint && firstId ? (
            <p className="tc-field__hint" data-testid="settings-connect-default-hint">
              {t("models.connect.defaultAfterSave", { name: firstId })}
            </p>
          ) : null}
          {showSetAsDefault ? (
            <label className="tc-settings-capabilities__item">
              <input
                checked={setAsDefault}
                data-testid="settings-connect-set-default"
                onChange={(event) => setSetAsDefault(event.target.checked)}
                type="checkbox"
              />
              <span>{t("models.setAsDefault")}</span>
            </label>
          ) : null}
          {saveFinished && !receipt?.success ? (
            <div role="status">
              {state.error ? <p className="tc-banner tc-banner--warning">{state.error}</p> : null}
              {state.warnings?.map((warning) => <p key={warning}>{warning}</p>)}
            </div>
          ) : null}

          <div className="tc-button-row tc-settings-form__actions">
            <button
              className="tc-button tc-button--ghost"
              data-testid="settings-connect-cancel"
              onClick={onClose}
              type="button"
            >
              {t("common.cancel")}
            </button>
            <button
              className="tc-button tc-button--primary"
              data-testid="settings-connect-save"
              onClick={handleSave}
              type="button"
            >
              {t("common.save")}
            </button>
          </div>
        </fieldset>
      </section>
    </div>
  );
}
