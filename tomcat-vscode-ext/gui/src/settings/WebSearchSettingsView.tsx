import { Fragment, useEffect, useId, useRef, useState, type ReactNode } from "react";
import type { SearchBackendView, SettingsIntent, SettingsStateSnapshot, VsCodeApiLike } from "../../../src/shared/settingsProtocol";
import { useT } from "../i18n/LocaleProvider";
import { SettingsShell } from "./SettingsShell";
import { send } from "./settingsShared";
import "./webSearchSettings.css";

type KeyAction = "save" | "clear";

function BackendRow({ row, index, enabledCount, busy, editor, onMove, onToggle, onEdit, onOpenModels }: {
  row: SearchBackendView;
  index: number;
  enabledCount: number;
  busy: boolean;
  editor: ReactNode;
  onMove(delta: number, button: HTMLButtonElement): void;
  onToggle(button: HTMLButtonElement): void;
  onEdit(action: KeyAction, trigger: HTMLButtonElement): void;
  onOpenModels(): void;
}) {
  const t = useT();
  const enabled = index >= 0;
  const shared = row.sharedModelIds.length > 0;
  const keepOne = enabled && enabledCount === 1;
  const canEditKey = Boolean(row.canSaveKey && row.keyEnv);
  const showModels = Boolean(row.model) || (row.canSaveKey && (!row.keyEnv || shared));
  const enterKey = canEditKey && !row.keySource;
  const showMenu = Boolean(row.keySource);
  const actionable = enterKey || showMenu || showModels;
  const canRemoveKey = canEditKey && !shared && row.keySource === "saved";
  const menuId = useId();
  const keyButton = useRef<HTMLButtonElement>(null);
  const keyLabel = t(showMenu ? "searchSettings.configured"
    : !actionable ? "searchSettings.keyMissing"
    : showModels && !enterKey ? "searchSettings.models" : "searchSettings.configure");
  const source = t(row.keySource === "environment" ? "searchSettings.keyEnvironment" : "searchSettings.keySaved");
  const editKey = (action: KeyAction) => { if (keyButton.current) onEdit(action, keyButton.current); };
  const details = [row.model ?? row.host, enabled && !row.keySource && t("searchSettings.skipped")].filter(Boolean).join(" · ");
  const label = { name: row.label };

  return <li className="tc-settings-card tc-search-settings__row" data-testid={`web-search-row-${row.id}`}>
    <div className="tc-search-settings__line">
      <div className="tc-search-settings__rank">{enabled && <>
        <span>{index + 1}</span>
        {([-1, 1] as const).map(delta => {
          const blocked = busy || index + delta < 0 || index + delta >= enabledCount;
          return <button key={delta} type="button" className="tc-icon-button"
            aria-label={t(delta < 0 ? "searchSettings.up" : "searchSettings.down", label)}
            aria-disabled={blocked}
            onClick={event => { if (!blocked) onMove(delta, event.currentTarget); }}>
            <span aria-hidden="true" className={`codicon codicon-arrow-${delta < 0 ? "up" : "down"}`} />
          </button>;
        })}
      </>}</div>
      <div className="tc-settings-card__identity">
        <strong>{row.label}</strong>
        {details && <span className="tc-settings-card__hosts">{details}</span>}
        {row.host && row.canSaveKey && !row.keyEnv && <span className="tc-settings-card__hosts">{t("searchSettings.configureHost", { host: row.host })}</span>}
        {!row.canSaveKey && row.keyEnv && !row.model && <span className="tc-settings-card__hosts">{t("searchSettings.keyUnavailable")}</span>}
      </div>
      <div className="tc-search-settings__actions">
        {actionable ? <button ref={keyButton} data-testid={`web-search-key-${row.id}`}
          className="tc-button tc-button--secondary tc-search-settings__key" type="button" disabled={busy}
          aria-label={`${keyLabel} · ${row.label}`} popoverTarget={showMenu ? menuId : undefined}
          onClick={enterKey ? () => editKey("save") : showMenu ? undefined : onOpenModels}>
          {keyLabel}
          {showMenu && <span aria-hidden="true" className="codicon codicon-chevron-down" />}
        </button> : <span className="tc-muted">{keyLabel}</span>}
        {showMenu && <div id={menuId} data-testid={`web-search-key-actions-${row.id}`} popover="auto"
          className="tc-session-dropdown tc-search-settings__key-actions">
          <p className="tc-search-settings__key-source">{source}{shared && <><br />{t("searchSettings.keyShared", { names: row.sharedModelIds.join(", ") })}</>}</p>
          {canEditKey && <button type="button" className="tc-session-item" disabled={busy}
            popoverTarget={menuId} popoverTargetAction="hide" onClick={() => editKey("save")}>{t("models.card.replaceKey")}</button>}
          {canRemoveKey && <button type="button" className="tc-session-item" disabled={busy}
            popoverTarget={menuId} popoverTargetAction="hide" onClick={() => editKey("clear")}>{t("searchSettings.clear")}…</button>}
          {showModels && <button type="button" className="tc-session-item" disabled={busy}
            popoverTarget={menuId} popoverTargetAction="hide" onClick={onOpenModels}>{t("searchSettings.models")}</button>}
        </div>}
        <button data-testid={`web-search-toggle-${row.id}`} type="button" role="switch"
          aria-label={t("searchSettings.enable", label)} aria-checked={enabled}
          className="tc-connector-tool tc-search-settings__switch"
          title={keepOne ? t("searchSettings.keepOne") : undefined} aria-disabled={busy || keepOne}
          onClick={event => { if (!busy && !keepOne) onToggle(event.currentTarget); }}>
          <span aria-hidden="true" className="tc-search-settings__track" />
        </button>
      </div>
    </div>
    {editor}
  </li>;
}

export function WebSearchSettingsView({ state, vscodeApi }: {
  state: SettingsStateSnapshot;
  vscodeApi: VsCodeApiLike<SettingsIntent>;
}) {
  const t = useT();
  const [pending, setPending] = useState<string>();
  const [editor, setEditor] = useState<{ id: string; action: KeyAction; trigger: HTMLButtonElement }>();
  const [draft, setDraft] = useState("");
  const receipt = state.webSearchReceipt;
  const focusAfterSave = useRef<{ button: HTMLButtonElement; id: string; label: string } | undefined>(undefined);
  const [announcement, setAnnouncement] = useState("");
  useEffect(() => {
    if (pending && (!state.ready || !state.webSearch || (receipt?.requestId === pending && receipt.phase !== "saving"))) {
      setPending(undefined);
      if (receipt?.phase === "saved") {
        editor?.trigger.focus();
        setEditor(undefined);
      }
    }
  }, [receipt, pending, state.ready, state.webSearch, editor]);
  useEffect(() => {
    const target = focusAfterSave.current;
    if (!target || pending || receipt?.phase === "saving") return;
    focusAfterSave.current = undefined;
    target.button.focus();
    if (receipt?.phase !== "saved") return;
    const index = state.webSearch?.order.indexOf(target.id) ?? -1;
    setAnnouncement(index < 0 ? t("searchSettings.disabled", { name: target.label }) : t("searchSettings.position", { name: target.label, index: index + 1 }));
  }, [receipt, pending, state.webSearch, t]);
  const settings = state.ready ? state.webSearch : undefined;
  const byId = new Map(settings?.backends.map(row => [row.id, row]));
  const order = settings?.order.filter(id => byId.has(id)) ?? [];
  const unknown = settings?.order.filter(id => !byId.has(id)) ?? [];
  const rows = [...order.map(id => byId.get(id)!), ...(settings?.backends.filter(row => !order.includes(row.id)) ?? [])];
  const busy = Boolean(pending || receipt?.phase === "saving");
  const saveOrder = (backends: string[]) => setPending(send(vscodeApi, { type: "setWebSearchRouting", data: { backends } }));
  const move = (index: number, delta: number) => {
    const next = [...order];
    [next[index], next[index + delta]] = [next[index + delta], next[index]];
    saveOrder(next);
  };
  const closeEditor = () => { editor?.trigger.focus(); setDraft(""); setEditor(undefined); };
  const openModels = () => send(vscodeApi, { type: "settings.ready", data: { route: "models" } });

  function renderKeyEditor(row: SearchBackendView) {
    if (editor?.id !== row.id) return null;
    const clearing = editor.action === "clear";
    return <form className="tc-settings-inline-key tc-search-settings__edit"
      onKeyDown={event => { if (event.key === "Escape" && !busy) closeEditor(); }}
      onSubmit={event => {
        event.preventDefault();
        if (busy) return;
        if (clearing) setPending(send(vscodeApi, { type: "clearSearchKey", data: { backendId: row.id } }));
        else if (draft.trim()) {
          setPending(send(vscodeApi, { type: "saveSearchKey", data: { backendId: row.id, value: draft.trim() } }));
          setDraft("");
        }
      }}>
      {clearing ? <span>{t("searchSettings.clearPrompt", { name: row.label })}</span> : <>
        {row.sharedModelIds.length > 0 && <small>{t("models.replaceAffected")} {row.sharedModelIds.join(", ")}</small>}
        <input type="password" autoComplete="off" autoFocus className="tc-input" aria-label={t("term.field.apiKey")}
          value={draft} disabled={busy} onChange={event => setDraft(event.target.value)} />
      </>}
      <button data-testid="web-search-key-submit" type="submit" className="tc-button tc-button--primary"
        disabled={busy || (!clearing && !draft.trim())}>{t(clearing ? "searchSettings.clear" : "common.save")}</button>
      <button type="button" className="tc-button tc-button--ghost" autoFocus={clearing} disabled={busy} onClick={closeEditor}>{t("common.cancel")}</button>
    </form>;
  }

  return <SettingsShell state={state} vscodeApi={vscodeApi}>
    <main className="tc-settings-shell__content tc-search-settings__content">
      <header className="tc-settings-shell__header"><div>
        <div role="tablist" aria-label={t("settings.tools")} className="tc-search-settings__tabs">
          <button type="button" role="tab" id="web-search-tab" aria-selected="true" aria-controls="web-search-panel" className="tc-chip tc-search-settings__tab">{t("searchSettings.title")}</button>
        </div>
      </div></header>
      <section id="web-search-panel" role="tabpanel" aria-labelledby="web-search-tab">
        <p>{t("searchSettings.description")}</p>
      {state.error && <div className="tc-banner tc-banner--warning" role="alert">{state.error}</div>}
      <div role="status" className={busy ? undefined : "tc-visually-hidden"}>{busy ? t("searchSettings.saving") : [state.status, announcement].filter(Boolean).join(" ")}</div>
      {!state.ready && !state.error && <p>{t("settings.waitingForConnection")}</p>}
      {state.ready && !state.capabilities.webSearchSettings && <div className="tc-banner tc-banner--warning">{t("searchSettings.unsupported")}</div>}
      {settings?.backends.length === 0 && <div className="tc-banner">{t("searchSettings.noPluginBackends")}</div>}
      {settings && !order.some(id => byId.get(id)?.keySource) && <div className="tc-banner tc-banner--warning">{t("searchSettings.noneAvailable")}</div>}
      {unknown.length > 0 && <div className="tc-banner tc-banner--warning">{t("searchSettings.unknown", { names: unknown.join(", ") })}</div>}
      {settings && <ol className="tc-search-settings__rows" aria-label={t("searchSettings.title")} aria-busy={busy}>
        {rows.map((row, rowIndex) => {
          const index = order.indexOf(row.id);
          const restoreFocus = (button: HTMLButtonElement) => {
            focusAfterSave.current = { button, id: row.id, label: row.label };
          };
          return <Fragment key={row.id}>
            {(rowIndex === 0 || rowIndex === order.length) && <li className="tc-search-settings__group" role="presentation">
              <h2>{t(index >= 0 ? "searchSettings.enabledGroup" : "searchSettings.disabledGroup")}</h2>
            </li>}
            <BackendRow row={row} index={index} enabledCount={order.length} busy={busy} editor={renderKeyEditor(row)}
              onOpenModels={openModels}
              onEdit={(action, trigger) => { setDraft(""); setEditor({ id: row.id, action, trigger }); }}
              onMove={(delta, button) => { restoreFocus(button); move(index, delta); }}
              onToggle={button => {
                restoreFocus(button);
                saveOrder(index >= 0 ? order.filter(id => id !== row.id) : [...order, row.id]);
              }} />
          </Fragment>;
        })}
      </ol>}
      </section>
    </main>
  </SettingsShell>;
}
