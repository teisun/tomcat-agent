import { useCallback, useEffect, useMemo, useState } from "react";

import type {
  SettingsHostFrame,
  SettingsIntent,
  SettingsModelView,
  SettingsRoute,
  SettingsStateSnapshot,
  VsCodeApiLike,
} from "../../../src/shared/settingsProtocol";
import type { Translator } from "../../../src/shared/i18n";
import { useT } from "../i18n/LocaleProvider";
import { ConnectProviderDialog, OTHER_ADDRESS, type ConnectTarget } from "./ConnectProviderDialog";
import { ConnectorsSettingsView } from "./ConnectorsSettingsView";
import { GeneralSettingsView } from "./GeneralSettingsView";
import { ModelEditDialog } from "./ModelEditDialog";
import { buildProviderCards, keyStatusLabel, type ProviderCard } from "./providerCards";
import type { ProviderPreset } from "./providerPresets";
import { fieldText, formatApiLabel, formatThinkingLabel, send } from "./settingsShared";
import { SettingsShell } from "./SettingsShell";
import { UtilityModelPicker } from "./UtilityModelPicker";

type SettingsDomAction = {
  kind: "clickTestId" | "setInputValue";
  testId?: string;
  value?: string;
};

function frameIsState(message: unknown): message is SettingsHostFrame {
  return (
    typeof message === "object" &&
    message !== null &&
    (message as { channel?: unknown }).channel === "state"
  );
}

function frameIsTestEvent(message: unknown): message is {
  channel: "event";
  content: {
    action?: SettingsDomAction;
    type: "__test.capture_dom" | "__test.dom_action";
  };
  messageId?: string;
} {
  return (
    typeof message === "object" &&
    message !== null &&
    (message as { channel?: unknown }).channel === "event" &&
    typeof (message as { content?: { type?: unknown } }).content?.type ===
      "string"
  );
}

function readTestIdRect(
  testId: string,
): { height: number; left: number; top: number; width: number } | undefined {
  const el = document.querySelector<HTMLElement>(`[data-testid="${testId}"]`);
  if (!el) {
    return undefined;
  }
  const rect = el.getBoundingClientRect();
  return {
    height: rect.height,
    left: rect.left,
    top: rect.top,
    width: rect.width,
  };
}

function buildSettingsDomSnapshot(): {
  html: string;
  rects: {
    aliasInput?: { height: number; left: number; top: number; width: number };
    keySlotBox?: { height: number; left: number; top: number; width: number };
    keySlotInput?: { height: number; left: number; top: number; width: number };
  };
} {
  return {
    html: document.getElementById("root")?.innerHTML ?? "",
    rects: {
      aliasInput: readTestIdRect("settings-model-alias"),
      keySlotBox: readTestIdRect("settings-key-slot-box"),
      keySlotInput: readTestIdRect("settings-key-slot-input"),
    },
  };
}

function runSettingsDomAction(action: SettingsDomAction | undefined): void {
  if (!action || !action.testId) {
    return;
  }
  const target = document.querySelector<HTMLElement>(
    `[data-testid="${action.testId}"]`,
  );
  if (!(target instanceof HTMLElement)) {
    return;
  }
  if (action.kind === "clickTestId") {
    target.click();
    return;
  }
  if (
    action.kind === "setInputValue"
    && (target instanceof HTMLInputElement
      || target instanceof HTMLTextAreaElement
      || target instanceof HTMLSelectElement)
  ) {
    const descriptor = Object.getOwnPropertyDescriptor(
      Object.getPrototypeOf(target),
      "value",
    );
    if (descriptor?.set) {
      descriptor.set.call(target, action.value ?? "");
    } else {
      target.value = action.value ?? "";
    }
    target.dispatchEvent(new Event("input", { bubbles: true }));
    target.dispatchEvent(new Event("change", { bubbles: true }));
  }
}

function buildServeVersionWarning(state: SettingsStateSnapshot, t: Translator): string | null {
  if (!state.ready) return null;
  const expected = fieldText(state.expectedCliVersion);
  const server = fieldText(state.serverVersion);
  if (!server) {
    return t("models.versionMissing");
  }
  if (expected && server !== expected) {
    return t("models.versionMismatch", { expected, server });
  }
  return null;
}

function buildModelDetails(model: SettingsModelView, t: Translator): Array<{ id: string; label: string; value: string }> {
  return [
    { id: "source", label: t("models.source"), value: t(model.source === "user" ? "models.user" : "models.builtin") },
    { id: "api", label: t("term.api"), value: formatApiLabel(model.api, t) },
    { id: "provider", label: t("models.provider"), value: model.provider },
    { id: "key", label: t("models.apiKeyEnv"), value: fieldText(model.apiKeyEnv) },
    { id: "url", label: t("term.baseUrl"), value: model.baseUrl ?? "" },
    { id: "thinking", label: t("thinking.title"), value: formatThinkingLabel(model.thinkingFormat, t) },
    { id: "context", label: t("models.contextWindow"), value: typeof model.contextWindow === "number" ? String(model.contextWindow) : "" },
    { id: "name", label: t("models.modelName"), value: model.modelName ?? "" },
  ].filter(entry => entry.value.trim().length > 0);
}

/** Deleting a card removes only Tomcat's saved key; an environment key with no user models leaves nothing to remove. */
function removeBlockedReason(card: ProviderCard, state: SettingsStateSnapshot, t: Translator): string | null {
  if (!state.capabilities.removeProvider) return t("models.removeProvider.unsupported");
  if (card.keySource === "environment" && !card.rows.some((row) => row.model.source === "user")) {
    return t("models.removeProvider.envKey", { name: card.keyEnv });
  }
  return null;
}

export function SettingsApp({
  vscodeApi,
  initialRoute = "models",
}: {
  vscodeApi: VsCodeApiLike<SettingsIntent>;
  initialRoute?: SettingsRoute;
}) {
  const t = useT();
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
    route: initialRoute,
  });
  const [connectTarget, setConnectTarget] = useState<ConnectTarget | null>(null);
  const [editingModelId, setEditingModelId] = useState<string | null>(null);
  const [deleteConfirmation, setDeleteConfirmation] = useState<string | null>(null);
  const [deletingModelId, setDeletingModelId] = useState<string | null>(null);
  const [openMenu, setOpenMenu] = useState<string | null>(null);
  const [removeCardKeyEnv, setRemoveCardKeyEnv] = useState<string | null>(null);
  const [replaceKeyEnv, setReplaceKeyEnv] = useState<string | null>(null);
  const [replaceKeyDraft, setReplaceKeyDraft] = useState("");
  const [keyEntryEnv, setKeyEntryEnv] = useState<string | null>(null);
  const [keyEntryDraft, setKeyEntryDraft] = useState("");

  useEffect(() => {
    const handleMessage = (event: MessageEvent<unknown>) => {
      if (frameIsTestEvent(event.data)) {
        if (event.data.content.type === "__test.capture_dom") {
          (vscodeApi as VsCodeApiLike<unknown>).postMessage({
            data: buildSettingsDomSnapshot(),
            messageId: event.data.messageId ?? `settings-dom-${Date.now()}`,
            type: "__test.dom_snapshot",
          });
          return;
        }
        if (event.data.content.type === "__test.dom_action") {
          runSettingsDomAction(event.data.content.action);
          return;
        }
      }
      if (!frameIsState(event.data)) {
        return;
      }
      setState(event.data.content);
    };
    window.addEventListener("message", handleMessage);
    send(vscodeApi, {
      data: {
        route: initialRoute,
      },
      type: "settings.ready",
    });
    return () => {
      window.removeEventListener("message", handleMessage);
    };
  }, [vscodeApi, initialRoute]);

  useEffect(() => {
    const receipt = state.modelRemovalReceipt;
    if (!receipt || receipt.modelId !== deletingModelId) {
      return;
    }
    setDeletingModelId(null);
    if (receipt.success) {
      setEditingModelId((current) => (current === receipt.modelId ? null : current));
    }
  }, [deletingModelId, state.modelRemovalReceipt]);

  useEffect(() => {
    if (!openMenu) return;
    const close = (event: Event) => {
      if (event instanceof KeyboardEvent && event.key !== "Escape") return;
      if (event instanceof MouseEvent && (event.target as Element | null)?.closest?.(".tc-settings-card__menu-anchor")) return;
      setOpenMenu(null);
    };
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", close);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", close);
    };
  }, [openMenu]);

  const presetLabel = useCallback((preset: ProviderPreset) => t(preset.labelKey), [t]);
  const { cards, connectable } = useMemo(
    () => buildProviderCards({
      defaultModelId: state.defaultModelId,
      models: state.models,
      presetLabel,
      utilityModelId: state.utilityModelId,
    }),
    [presetLabel, state.defaultModelId, state.models, state.utilityModelId],
  );
  const closeConnect = useCallback(() => setConnectTarget(null), []);
  const closeEdit = useCallback(() => setEditingModelId(null), []);
  const editingModel = state.models.find((model) => model.id === editingModelId) ?? null;
  const removeCard = cards.find((card) => card.keyEnv === removeCardKeyEnv) ?? null;
  const replaceCard = cards.find((card) => card.keyEnv === replaceKeyEnv) ?? null;
  const canSetDefault = state.capabilities.setDefaultModel === true;
  const connectSupported = state.capabilities.listRemoteModels === true
    && state.capabilities.upsertModels === true
    && state.capabilities.removeProvider === true
    && state.capabilities.setDefaultModelIfUnset === true;
  const serveVersionWarning = buildServeVersionWarning(state, t);

  function confirmDelete() {
    if (!deleteConfirmation || deletingModelId) {
      return;
    }
    setDeletingModelId(deleteConfirmation);
    setDeleteConfirmation(null);
    send(vscodeApi, {
      data: { modelId: deleteConfirmation },
      type: "removeModel",
    });
  }

  function saveCardKey(card: ProviderCard) {
    const value = keyEntryDraft.trim();
    if (!value) return;
    const firstModelId = card.rows[0]?.model.id;
    send(vscodeApi, {
      data: { envName: card.keyEnv, value, ...(firstModelId ? { defaultModelId: firstModelId } : {}) },
      type: "setProviderKey",
    });
    setKeyEntryEnv(null);
    setKeyEntryDraft("");
  }

  function saveReplacementKey() {
    const value = replaceKeyDraft.trim();
    if (!replaceCard || !value) return;
    send(vscodeApi, { data: { envName: replaceCard.keyEnv, value }, type: "setProviderKey" });
    setReplaceKeyEnv(null);
    setReplaceKeyDraft("");
  }

  function confirmRemoveProvider() {
    if (!removeCard) return;
    send(vscodeApi, { data: { keyEnv: removeCard.keyEnv }, type: "removeProvider" });
    setRemoveCardKeyEnv(null);
  }

  function renderRow(card: ProviderCard, row: ProviderCard["rows"][number]) {
    const { model } = row;
    const details = buildModelDetails(model, t);
    return (
      <li className="tc-settings-card__row" data-testid={`settings-model-row-${model.id}`} key={model.id}>
        <div className="tc-settings-card__model">
          <strong>{model.id}</strong>
          {canSetDefault && row.isDefault ? (
            <span className="tc-chip" data-testid={`settings-default-${model.id}`}>{t("models.default")}</span>
          ) : null}
          {row.isUtility ? (
            <span className="tc-chip" data-testid={`settings-utility-${model.id}`}>{t("models.utilityChip")}</span>
          ) : null}
          <span className="tc-settings-card__protocol">{formatApiLabel(model.api, t)}</span>
        </div>
        <div className="tc-settings-model__actions">
          {canSetDefault && !row.isDefault && card.keyPresent ? (
            <button
              className="tc-button tc-button--secondary"
              data-testid={`settings-make-default-${model.id}`}
              onClick={() => send(vscodeApi, { data: { modelId: model.id }, type: "setDefaultModel" })}
              type="button"
            >
              {t("models.makeDefault")}
            </button>
          ) : null}
          <div className="tc-settings-model__tooltip-anchor">
            <button
              aria-label={t("models.details", { name: model.id })}
              className="tc-settings-model__info"
              type="button"
            >
              <span aria-hidden="true" className="codicon codicon-info" />
            </button>
            <div className="tc-settings-model__tooltip" role="tooltip">
              <dl className="tc-settings-model__tooltip-list">
                {details.map((entry) => (
                  <div className="tc-settings-model__tooltip-row" key={`${model.id}-${entry.id}`}>
                    <dt>{entry.label}</dt>
                    <dd>{entry.value}</dd>
                  </div>
                ))}
              </dl>
            </div>
          </div>
          <button
            className="tc-button tc-button--secondary"
            data-testid={`settings-edit-${model.id}`}
            onClick={() => setEditingModelId(model.id)}
            type="button"
          >
            {t("models.edit")}
          </button>
          {model.source === "user" ? (
            <button
              className="tc-button tc-button--secondary"
              data-testid={`settings-delete-${model.id}`}
              disabled={!state.capabilities.removeModel || deletingModelId !== null}
              onClick={() => setDeleteConfirmation(model.id)}
              type="button"
            >
              {deletingModelId === model.id ? t("models.deleting", { name: model.id }) : t("models.delete")}
            </button>
          ) : null}
        </div>
      </li>
    );
  }

  function renderCard(card: ProviderCard) {
    const blocked = removeBlockedReason(card, state, t);
    const menuOpen = openMenu === card.keyEnv;
    return (
      <article className="tc-settings-card" data-testid={`settings-card-${card.keyEnv}`} key={card.keyEnv}>
        <div className="tc-settings-card__header">
          <div className="tc-settings-card__identity">
            <strong className="tc-settings-card__title">{card.title}</strong>
            {card.hosts.length > 0 ? (
              <span className="tc-settings-card__hosts">{card.hosts.join(" · ")}</span>
            ) : null}
          </div>
          <div className="tc-settings-card__status">
            <span
              className={`tc-settings-card__key${card.keyPresent ? "" : " tc-settings-card__key--missing"}`}
              data-testid={`settings-card-key-${card.keyEnv}`}
            >
              {keyStatusLabel(card, t)}
            </span>
            {!card.keyPresent ? (
              <button
                className="tc-button tc-button--secondary"
                data-testid={`settings-card-fill-key-${card.keyEnv}`}
                disabled={!state.capabilities.setProviderKey}
                onClick={() => {
                  setKeyEntryEnv(card.keyEnv);
                  setKeyEntryDraft("");
                }}
                type="button"
              >
                {t("models.card.fillKey")}
              </button>
            ) : null}
            <div className="tc-settings-card__menu-anchor">
              <button
                aria-expanded={menuOpen}
                aria-haspopup="menu"
                aria-label={t("models.card.menu", { name: card.title })}
                className="tc-icon-button"
                data-testid={`settings-card-menu-${card.keyEnv}`}
                onClick={() => setOpenMenu(menuOpen ? null : card.keyEnv)}
                type="button"
              >
                <span aria-hidden="true" className="codicon codicon-ellipsis" />
              </button>
              {menuOpen ? (
                <div className="tc-session-dropdown tc-settings-card__menu" role="menu">
                  <button
                    className="tc-session-item"
                    data-testid={`settings-card-replace-key-${card.keyEnv}`}
                    disabled={!state.capabilities.setProviderKey}
                    onClick={() => {
                      setOpenMenu(null);
                      setReplaceKeyEnv(card.keyEnv);
                      setReplaceKeyDraft("");
                    }}
                    role="menuitem"
                    type="button"
                  >
                    {t("models.card.replaceKey")}
                  </button>
                  <button
                    className="tc-session-item"
                    data-testid={`settings-card-remove-${card.keyEnv}`}
                    disabled={blocked !== null}
                    onClick={() => {
                      setOpenMenu(null);
                      setRemoveCardKeyEnv(card.keyEnv);
                    }}
                    role="menuitem"
                    title={blocked ?? undefined}
                    type="button"
                  >
                    {t("models.card.remove")}
                  </button>
                  {blocked ? <small className="tc-settings-card__menu-hint">{blocked}</small> : null}
                </div>
              ) : null}
            </div>
          </div>
        </div>
        {keyEntryEnv === card.keyEnv ? (
          <div className="tc-settings-inline-key">
            <input
              aria-label={t("term.field.apiKey")}
              autoComplete="off"
              autoFocus
              className="tc-input"
              data-testid={`settings-card-key-input-${card.keyEnv}`}
              onChange={(event) => setKeyEntryDraft(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") saveCardKey(card);
                if (event.key === "Escape") setKeyEntryEnv(null);
              }}
              placeholder={t("models.card.keyPlaceholder")}
              type="password"
              value={keyEntryDraft}
            />
            <button
              className="tc-button tc-button--primary"
              data-testid={`settings-card-key-save-${card.keyEnv}`}
              disabled={!keyEntryDraft.trim()}
              onClick={() => saveCardKey(card)}
              type="button"
            >
              {t("common.save")}
            </button>
            <button className="tc-button tc-button--ghost" onClick={() => setKeyEntryEnv(null)} type="button">
              {t("common.cancel")}
            </button>
          </div>
        ) : null}
        <ul className="tc-settings-card__rows">
          {card.rows.map((row) => renderRow(card, row))}
        </ul>
        {connectSupported ? (
          <button
            className="tc-settings-card__add"
            data-testid={`settings-card-add-model-${card.keyEnv}`}
            onClick={() => setConnectTarget({ keyEnv: card.keyEnv, kind: "card" })}
            type="button"
          >
            {t("models.card.addModel")}
          </button>
        ) : null}
      </article>
    );
  }

  if (state.route === "general") {
    return <GeneralSettingsView state={state} vscodeApi={vscodeApi} />;
  }

  if (state.route === "connectors") {
    return <ConnectorsSettingsView state={state} vscodeApi={vscodeApi} />;
  }

  return (
    <SettingsShell state={state} vscodeApi={vscodeApi}>
      <main className="tc-settings-shell__content">
        <header className="tc-settings-shell__header">
          <div>
            <h1>{t("models.title")}</h1>
            <p>{t("models.description")}</p>
          </div>
        </header>

        {serveVersionWarning ? (
          <div className="tc-banner tc-banner--warning">
            {serveVersionWarning}
          </div>
        ) : null}
        {state.error ? (
          <div className="tc-banner tc-banner--warning">{state.error}</div>
        ) : null}
        {state.warnings?.map((warning) => (
          <div key={warning} className="tc-banner tc-banner--warning">
            {warning}
          </div>
        ))}
        {state.status ? <div className="tc-banner">{state.status}</div> : null}

        {!state.ready ? (
          <section className="tc-empty-state"><p>{t("settings.waitingForConnection")}</p></section>
        ) : !state.capabilities.listModels ? (
          <section className="tc-empty-state">
            <h2>{t("models.unavailable")}</h2>
            <p>{t("models.unavailableHint")}</p>
          </section>
        ) : (
          <div className="tc-settings-groups">
            {state.utilityModelId !== undefined ? (
              <UtilityModelPicker
                canSet={state.capabilities.setUtilityModel === true}
                cards={cards}
                models={state.models}
                onSelect={(modelId) => send(vscodeApi, { data: { modelId }, type: "setUtilityModel" })}
                utilityModelId={state.utilityModelId}
              />
            ) : null}

            <section className="tc-settings-group">
              <div className="tc-settings-providers__header">
                <h2 className="tc-settings-group__title">{t("models.providers")}</h2>
                <button
                  className="tc-button tc-button--secondary"
                  data-testid="settings-connect-provider"
                  disabled={!connectSupported}
                  onClick={() => setConnectTarget({ kind: "new", preset: connectable[0]?.provider ?? OTHER_ADDRESS })}
                  title={connectSupported ? undefined : t("models.connect.unsupported")}
                  type="button"
                >
                  {t("models.connect")}
                </button>
              </div>
              {!connectSupported ? (
                <small className="tc-field__hint" data-testid="settings-connect-unsupported">
                  {t("models.connect.unsupported")}
                </small>
              ) : null}
              {cards.map(renderCard)}
              <p className="tc-settings-connectable" data-testid="settings-connectable">
                <span>{t("models.connectable")}</span>
                {[...connectable.map((preset) => ({ id: preset.provider, label: presetLabel(preset) })),
                  { id: OTHER_ADDRESS, label: t("models.preset.other") }].map((entry, index) => (
                  <span key={entry.id}>
                    {index > 0 ? " · " : " "}
                    <button
                      className="tc-link-button"
                      data-testid={`settings-connectable-${entry.id}`}
                      disabled={!connectSupported}
                      onClick={() => setConnectTarget({ kind: "new", preset: entry.id })}
                      type="button"
                    >
                      {entry.label}
                    </button>
                  </span>
                ))}
              </p>
            </section>
          </div>
        )}

        {connectTarget ? (
          <ConnectProviderDialog
            cards={cards}
            key={connectTarget.kind === "card" ? `card:${connectTarget.keyEnv}` : `new:${connectTarget.preset}`}
            onClose={closeConnect}
            presetLabel={presetLabel}
            state={state}
            target={connectTarget}
            vscodeApi={vscodeApi}
          />
        ) : null}
        {editingModel ? (
          <ModelEditDialog
            key={editingModel.id}
            model={editingModel}
            onClose={closeEdit}
            onDelete={setDeleteConfirmation}
            state={state}
            vscodeApi={vscodeApi}
          />
        ) : null}
        {deleteConfirmation ? (
          <div className="tc-settings-modal" role="presentation">
            <section
              aria-labelledby="delete-model-title"
              aria-modal="true"
              className="tc-card tc-settings-modal__card"
              data-testid="settings-delete-model-confirmation"
              role="alertdialog"
            >
              <div className="tc-settings-modal__header">
                <div>
                  <h3 id="delete-model-title">{t("models.deleteQuestion", { name: deleteConfirmation })}</h3>
                  <p>{t("models.deleteHint")}</p>
                </div>
              </div>
              <div className="tc-button-row tc-settings-form__actions">
                <button
                  className="tc-button tc-button--ghost"
                  onClick={() => setDeleteConfirmation(null)}
                  type="button"
                >
                  {t("common.cancel")}
                </button>
                <button
                  className="tc-button tc-button--danger"
                  data-testid="settings-delete-model-confirm"
                  onClick={confirmDelete}
                  type="button"
                >
                  {t("models.deleteAction")}
                </button>
              </div>
            </section>
          </div>
        ) : null}
        {removeCard ? (
          <div className="tc-settings-modal" role="presentation">
            <section
              aria-labelledby="remove-provider-title"
              aria-modal="true"
              className="tc-card tc-settings-modal__card"
              data-testid="settings-remove-provider-confirmation"
              role="alertdialog"
            >
              <div className="tc-settings-modal__header">
                <div>
                  <h3 id="remove-provider-title">{t("models.removeProvider.title", { name: removeCard.title })}</h3>
                </div>
              </div>
              <div className="tc-settings-form">
                <p>{t("models.removeProvider.body")}</p>
                <ul>
                  {removeCard.rows.map((row) => <li key={row.model.id}>{row.model.id}</li>)}
                </ul>
                {removeCard.keySource === "environment" ? (
                  <p>{t("models.removeProvider.envKey", { name: removeCard.keyEnv })}</p>
                ) : null}
                <p>{t("models.removeProvider.hint")}</p>
                <div className="tc-button-row tc-settings-form__actions">
                  <button className="tc-button tc-button--ghost" onClick={() => setRemoveCardKeyEnv(null)} type="button">
                    {t("common.cancel")}
                  </button>
                  <button
                    className="tc-button tc-button--danger"
                    data-testid="settings-remove-provider-confirm"
                    onClick={confirmRemoveProvider}
                    type="button"
                  >
                    {t("models.card.remove")}
                  </button>
                </div>
              </div>
            </section>
          </div>
        ) : null}
        {replaceCard ? (
          <div className="tc-settings-modal" role="presentation">
            <section
              aria-labelledby="replace-key-title"
              aria-modal="true"
              className="tc-card tc-settings-modal__card"
              data-testid="settings-replace-key-dialog"
              role="dialog"
            >
              <div className="tc-settings-modal__header">
                <div>
                  <h3 id="replace-key-title">{t("models.replaceKey.title", { name: replaceCard.title })}</h3>
                </div>
              </div>
              <div className="tc-settings-form">
                <p>{t("models.replaceAffected")}</p>
                <ul>
                  {replaceCard.rows.map((row) => <li key={row.model.id}>{row.model.id}</li>)}
                </ul>
                <input
                  aria-label={t("term.field.apiKey")}
                  autoComplete="off"
                  autoFocus
                  className="tc-input"
                  data-testid="settings-replace-key-input"
                  onChange={(event) => setReplaceKeyDraft(event.target.value)}
                  placeholder={t("models.card.keyPlaceholder")}
                  type="password"
                  value={replaceKeyDraft}
                />
                <div className="tc-button-row tc-settings-form__actions">
                  <button className="tc-button tc-button--ghost" onClick={() => setReplaceKeyEnv(null)} type="button">
                    {t("common.cancel")}
                  </button>
                  <button
                    className="tc-button tc-button--primary"
                    data-testid="settings-replace-key-save"
                    disabled={!replaceKeyDraft.trim()}
                    onClick={saveReplacementKey}
                    type="button"
                  >
                    {t("models.replaceKey.action")}
                  </button>
                </div>
              </div>
            </section>
          </div>
        ) : null}
      </main>
    </SettingsShell>
  );
}
