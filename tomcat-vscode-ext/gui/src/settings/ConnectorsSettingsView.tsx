import { useEffect, useMemo, useRef, useState } from "react";
import { SettingsShell } from "./SettingsShell";
import { useLocale, useT } from "../i18n/LocaleProvider";
import { pluralKey, type Translator, type Locale, type MessageKey } from "../../../src/shared/i18n";

import type {
  ConnectorInput,
  ConnectorToolView,
  ConnectorView,
} from "../../../src/shared/connectorsProtocol";
import type {
  SettingsIntent,
  SettingsConnectorReloadReceipt,
  SettingsStateSnapshot,
  VsCodeApiLike,
} from "../../../src/shared/settingsProtocol";

function send(
  vscodeApi: VsCodeApiLike<SettingsIntent>,
  type: SettingsIntent["type"],
  data?: unknown,
): string {
  const messageId = `connector-${type}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  vscodeApi.postMessage({
    messageId,
    type,
    ...(data === undefined ? {} : { data }),
  } as SettingsIntent);
  return messageId;
}

function statusLabel(connector: ConnectorView, busy: boolean, t: Translator): string {
  if (busy || connector.state === "connecting") {
    return connector.state === "connecting" && connector.attempt && connector.recovery
      ? t("connector.reconnectingAttempt", { attempt: connector.attempt, limit: connector.recovery.maxAttempts })
      : t("connector.state.connecting");
  }
  return t(`connector.state.${connector.state}`);
}

function statusClass(connector: ConnectorView): string {
  return `tc-connector-status tc-connector-status--${connector.state}`;
}

function reloadFeedback(connector: ConnectorView, receipt: SettingsConnectorReloadReceipt | undefined, t: Translator, locale: Locale): string | null {
  if (connector.compatibilityError) return connector.compatibilityError;
  if (receipt?.phase === "unknown" || receipt?.reason === "superseded" || receipt?.reason === "removed") return receipt.message ?? t("connector.reconnectUnknown");
  if (receipt?.phase === "failed" || connector.state === "failed") {
    const detail = receipt?.message ?? connector.error ?? t("connector.checkSettings");
    return connector.attempt ? t(pluralKey(locale, "connector.reconnectAttempts.other", connector.attempt), { count: connector.attempt, detail }) : t("connector.reconnectFailed", { detail });
  }
  return receipt?.phase === "succeeded" && connector.state === "connected" ? t("connector.reconnected") : connector.error ?? null;
}

function configurationPath(connector: ConnectorView): string | null {
  return connector.configPath ?? null;
}
function toolToggleKey(configKey: string, rawName: string): string {
  return JSON.stringify([configKey, rawName]);
}


function configurationPathForScope(
  state: SettingsStateSnapshot,
  scope: "global" | "workspace",
): string | null {
  const configured = scope === "global"
    ? state.connectorConfigPaths?.global
    : state.connectorConfigPaths?.workspace;
  return configured?.display ?? null;
}

function authenticationLabel(connector: ConnectorView, t: Translator): string {
  if (connector.auth === "oauth" || connector.oauthConfigured) {
    return t("term.oauth2");
  }
  if (connector.auth === "bearer") {
    return t("term.field.bearerToken");
  }
  return t("connector.auth.none");
}

function ConnectorConfigPathLink({
  onClick,
  path,
  testId,
}: {
  onClick: () => void;
  path: string;
  testId?: string;
}) {
  return (
    <button className="tc-connector-config-link tc-inline-path" data-testid={testId} onClick={onClick} title={path} type="button">
      <span aria-hidden="true" className="tc-inline-path__icon codicon codicon-file" />
      <span className="tc-inline-path__label tc-connector-config-link__label">{path}</span>
    </button>
  );
}

export function ConnectorsSettingsView({
  state,
  vscodeApi,
}: {
  state: SettingsStateSnapshot;
  vscodeApi: VsCodeApiLike<SettingsIntent>;
}) {
  const t = useT();
  const locale = useLocale();
  const connectors = state.connectors ?? [];
  const [selectedConfigKey, setSelectedConfigKey] = useState<string | null>(null);
  const selected = connectors.find((connector) => connector.configKey === selectedConfigKey) ?? null;
  const [reloadClicks, setReloadClicks] = useState<Record<string, string>>({});
  const reloadLocks = useRef(new Map<string, string>());
  const [tools, setTools] = useState<ConnectorToolView[]>([]);
  const [toolToggleRequests, setToolToggleRequests] = useState<Record<string, string>>({});
  const [lastToolToggleKey, setLastToolToggleKey] = useState<string | null>(null);
  const [showAdd, setShowAdd] = useState(false);
  const [transport, setTransport] = useState<"stdio" | "http">("stdio");
  const [url, setUrl] = useState("");
  const [command, setCommand] = useState("");
  const [args, setArgs] = useState("");
  const [name, setName] = useState("");
  const [authMode, setAuthMode] = useState<"oauth" | "bearer" | "none">("none");
  const [bearerToken, setBearerToken] = useState("");
  const [customHeaders, setCustomHeaders] = useState("");
  const [formError, setFormError] = useState<string | { key: MessageKey } | null>(null);
  const [envText, setEnvText] = useState("");
  const [scope, setScope] = useState<"workspace" | "global">("global");
  const [submissionId, setSubmissionId] = useState<string | null>(null);
  const [clientId, setClientId] = useState("");
  const [isSubmitting, setIsSubmitting] = useState(false);
  const submitLock = useRef(false);
  const trustLock = useRef<string | null>(null);
  const [loginRequest, setLoginRequest] = useState<{ configKey: string; requestId: string } | null>(null);
  const loginBusy = Boolean(loginRequest && loginRequest.configKey === selectedConfigKey);

  function reloadBusy(connector: ConnectorView): boolean {
    const local = reloadClicks[connector.configKey];
    const receipt = state.connectorReloads?.[connector.configKey];
    if (local && local !== receipt?.requestId) return true;
    if (receipt?.phase === "pending" || receipt?.phase === "accepted") return true;
    if (receipt?.phase === "unknown" || receipt?.phase === "failed") return false;
    return connector.state === "connecting";
  }
  function sourceToggleBusy(configKey: string): boolean {
    return Boolean(toolToggleRequests[configKey]);
  }

  const selectedBusy = selected ? reloadBusy(selected) : false;
  const catalog = state.connectorToolsIdentity;
  const toolsAvailable = Boolean(selected && !selectedBusy && selected.state === "connected"
    && state.selectedConnector === selected.configKey && catalog && catalog.configKey === selected.configKey
    && catalog.generation === selected.generation && catalog.attempt === selected.attempt);
  const selectedFeedback = selected ? reloadFeedback(selected, state.connectorReloads?.[selected.configKey], t, locale) : null;
  const canToggleTools = state.capabilities.connectorCapabilities?.toggle === true;
  const lastToolToggle = lastToolToggleKey && selected
    ? state.connectorToolToggles?.[lastToolToggleKey]
    : undefined;
  const toolToggleFeedback = lastToolToggle && lastToolToggle.configKey === selected?.configKey
    && (!lastToolToggle.configSaved || !lastToolToggle.runtimeApplied)
    ? lastToolToggle.error ?? t("connector.updateToolFailed")
    : null;
  const retryableToolToggle = Boolean(lastToolToggle && lastToolToggle.configKey === selected?.configKey
    && (lastToolToggle.configSaved === undefined
      || (lastToolToggle.configSaved === true && lastToolToggle.runtimeApplied === false)));
  const canRetryToolToggle = retryableToolToggle && Boolean(selected && !selected.overridden
    && selected.state === "connected" && !selectedBusy && canToggleTools
    && !sourceToggleBusy(selected.configKey));

  useEffect(() => {
    setTools(toolsAvailable ? state.connectorTools ?? [] : []);
  }, [toolsAvailable, state.connectorTools, selected?.generation, selected?.attempt]);

  useEffect(() => {
    let changed = false;
    for (const [key, requestId] of reloadLocks.current) {
      const receipt = state.connectorReloads?.[key];
      if (!connectors.some((connector) => connector.configKey === key)
        || (receipt?.requestId === requestId && receipt.phase !== "pending" && receipt.phase !== "accepted")) {
        reloadLocks.current.delete(key);
        changed = true;
      }
    }
    if (changed) setReloadClicks(Object.fromEntries(reloadLocks.current));
  }, [connectors, state.connectorReloads]);

  useEffect(() => {
    const receipt = state.connectorReceipt;
    if (!isSubmitting || !submissionId || receipt?.requestId !== submissionId) {
      return;
    }
    setIsSubmitting(false);
    submitLock.current = false;
    setSubmissionId(null);
    if (receipt.configSaved) {
      setShowAdd(false);
      return;
    }
    setFormError(receipt.error ?? { key: "connector.addFailed" });
  }, [isSubmitting, submissionId, state.connectorReceipt]);
  useEffect(() => {
    if (!state.ready) {
      setToolToggleRequests((current) => Object.keys(current).length === 0 ? current : {});
      return;
    }
    setToolToggleRequests((current) => {
      const receipts = Object.values(state.connectorToolToggles ?? {});
      const pending = Object.entries(current).filter(([configKey, requestId]) =>
        !receipts.some((receipt) => receipt.configKey === configKey && receipt.requestId === requestId),
      );
      return pending.length === Object.keys(current).length ? current : Object.fromEntries(pending);
    });
  }, [state.connectorToolToggles, state.ready]);

  useEffect(() => {
    if (selectedConfigKey && !connectors.some((connector) => connector.configKey === selectedConfigKey)) setSelectedConfigKey(null);
  }, [connectors, selectedConfigKey]);

  useEffect(() => {
    if (!state.ready || (state.connectorLogin?.requestId === loginRequest?.requestId && state.connectorLogin?.phase === "settled")) {
      setLoginRequest(null);
    }
  }, [state.ready, state.connectorLogin, loginRequest?.requestId]);

  const groups = useMemo(() => {
    const order: ConnectorView["state"][] = [
      "connected",
      "awaiting_project_trust",
      "pending",
      "connecting",
      "needs_authorization",
      "failed",
      "disconnected",
    ];
    return order
      .map((group) => ({
        label: t(`connector.state.${group}`),
        items: connectors.filter((connector) => connector.state === group),
        state: group,
      }))
      .filter((group) => group.items.length > 0);
  }, [connectors, t]);

  useEffect(() => {
    if (!state.ready || state.connectorProject?.trusted || state.error) trustLock.current = null;
  }, [state.ready, state.connectorProject?.trusted, state.error]);

  function requestProjectTrust(): void {
    const root = state.connectorProject?.root;
    if (!root || state.connectorProject?.trusted !== false || trustLock.current === root) return;
    trustLock.current = root;
    send(vscodeApi, "trustProject", { projectRoot: root });
  }

  function openDetail(connector: ConnectorView): void {
    setSelectedConfigKey(connector.configKey);
    setTools([]);
    // Register the viewed source even while it is recovering. The Host defers
    // the catalog read until Connected and then refreshes this selection.
    if (!connector.overridden) {
      send(vscodeApi, "listConnectorTools", { name: connector.name, configKey: connector.configKey });
    }
  }

  function reload(connector: ConnectorView): void {
    if (sourceToggleBusy(connector.configKey) || reloadLocks.current.has(connector.configKey) || reloadBusy(connector)) return;
    reloadLocks.current.set(connector.configKey, "sending");
    const requestId = send(vscodeApi, "reloadConnector", { name: connector.name, configKey: connector.configKey });
    reloadLocks.current.set(connector.configKey, requestId);
    setReloadClicks(Object.fromEntries(reloadLocks.current));
  }

  function openAdd(): void {
    setScope("global");
    setFormError(null);
    setIsSubmitting(false);
    submitLock.current = false;
    setShowAdd(true);
  }

  function requestToolToggle(rawName: string, enabled: boolean, retry = false): void {
    if (!selected || selected.overridden || !canToggleTools || sourceToggleBusy(selected.configKey)) {
      return;
    }
    if ((!retry && !toolsAvailable) || (retry && (selectedBusy || selected.state !== "connected"))) {
      return;
    }
    const key = toolToggleKey(selected.configKey, rawName);
    const requestId = send(vscodeApi, "setConnectorToolEnabled", {
      name: selected.name,
      configKey: selected.configKey,
      rawName,
      enabled,
    });
    setToolToggleRequests((current) => ({ ...current, [selected.configKey]: requestId }));
    setLastToolToggleKey(key);
  }

  function toggleTool(tool: ConnectorToolView): void {
    requestToolToggle(tool.rawName, !tool.enabled);
  }

  function submitAdd(): void {
    if (submitLock.current) return;
    const trimmedName = name.trim();
    if (!trimmedName) {
      setFormError({ key: "connector.nameRequired" });
      return;
    }
    if (transport === "http" && !/^https?:\/\//i.test(url.trim())) {
      setFormError({ key: "connector.urlRequired" });
      return;
    }
    if (transport === "stdio" && !command.trim()) {
      setFormError({ key: "connector.commandRequired" });
      return;
    }

    const headers = customHeaders.split("\n").reduce<Record<string, string>>((result, line) => {
      const separator = line.indexOf(":");
      if (separator > 0) {
        const key = line.slice(0, separator).trim();
        const value = line.slice(separator + 1).trim();
        if (key && value) {
          result[key] = value;
        }
      }
      return result;
    }, {});
    if (authMode === "bearer") {
      if (!bearerToken.trim()) {
        setFormError({ key: "connector.tokenRequired" });
        return;
      }
      headers.Authorization = `Bearer ${bearerToken.trim()}`;
    }

    const env = envText.split("\n").reduce<Record<string, string>>((result, line) => {
      const separator = line.indexOf("=");
      if (separator > 0) {
        const key = line.slice(0, separator).trim();
        if (key) {
          result[key] = line.slice(separator + 1);
        }
      }
      return result;
    }, {});
    const input: ConnectorInput = transport === "http"
      ? {
          name: trimmedName,
          type: "mcp",
          transport,
          url: url.trim(),
          headers,
          auth: authMode,
          oauth: authMode === "oauth" ? { clientId: clientId.trim() || undefined } : undefined,
          scope,
        }
      : {
          name: trimmedName,
          type: "mcp",
          transport,
          command: command.trim(),
          args: args.trim() ? args.trim().split(/\s+/) : [],
          env,
          scope,
        };
    const trustProject = scope === "workspace" && state.connectorProject?.trusted === false;
    if (trustProject && state.capabilities.connectorCapabilities?.trustProject !== true) {
      setFormError({ key: "connector.trustUpgrade" });
      return;
    }
    const requestId = send(vscodeApi, "addConnector", { connector: input, ...(trustProject ? { trustProject: true } : {}) });
    submitLock.current = true;
    setSubmissionId(requestId);
    setIsSubmitting(true);
    setFormError(null);
  }

  return (
    <SettingsShell state={state} vscodeApi={vscodeApi}>
      <main className="tc-settings-shell__content">
        <header className="tc-settings-shell__header">
          <div>
            <h1>{t("settings.connectors")}</h1>
            <p>{t("connector.description")}</p>
          </div>
          <button className="tc-button tc-button--secondary" data-testid="connector-add-open" onClick={openAdd} type="button">{t("connector.addOpen")}</button>
        </header>
        {state.error ? <div className="tc-banner tc-banner--warning">{state.error}</div> : null}
        {state.status ? <div className="tc-banner">{state.status}</div> : null}
        <div className="tc-banner tc-settings-restart-hint" role="status">
          {t("connector.restartHint")}
        </div>
        {groups.length === 0 ? (
          <section className="tc-empty-state">
            <h2>{t("connector.empty")}</h2>
            <p>{t("connector.emptyHint")}</p>
          </section>
        ) : groups.map((group) => (
          <section className="tc-settings-group" key={group.state}>
            <div className="tc-settings-group__heading">
              <h2 className="tc-settings-group__title">{group.label}</h2>
              {group.state === "awaiting_project_trust" && state.connectorProject?.trusted === false ? (
                <button className="tc-button tc-button--secondary" data-testid="connector-trust-project" disabled={state.connectorTrustPending || !state.capabilities.connectorCapabilities?.trustProject} onClick={requestProjectTrust} type="button">{t(state.connectorTrustPending ? "connector.trusting" : "connector.trust")}</button>
              ) : null}
            </div>
            {group.state === "awaiting_project_trust" && state.connectorProject ? <div className="tc-settings-group__project-path" title={state.connectorProject.root}>{state.connectorProject.root}</div> : null}
            <div className="tc-connector-list">
              {group.items.map((connector) => (
                <button className="tc-connector-card" data-testid={`connector-card-${connector.name}`} key={connector.configKey} onClick={() => openDetail(connector)} type="button">
                  {reloadBusy(connector) || connector.state === "connecting" ? <span className="tc-spinner tc-connector-spinner" aria-hidden="true" /> : <span className={statusClass(connector)} aria-hidden="true">●</span>}
                  <span className="tc-connector-card__body">
                    <strong>{connector.name}</strong>
                    <span role="status" aria-live={selected?.configKey === connector.configKey ? "off" : "polite"}>{statusLabel(connector, reloadBusy(connector), t)} · {connector.overridden ? t("connector.overridden") : connector.state === "connected" && !reloadBusy(connector) ? t("connector.enabledCount.other", { count: connector.toolCount, transport: t(`term.transport.${connector.transport}`) }) : t(`term.transport.${connector.transport}`)}</span>
                    {reloadFeedback(connector, state.connectorReloads?.[connector.configKey], t, locale) ? <span className="tc-connector-feedback">{reloadFeedback(connector, state.connectorReloads?.[connector.configKey], t, locale)}</span> : null}
                  </span>
                  <span className="tc-connector-card__source">{connector.overridden ? t("connector.overriddenSuffix", { source: t(`connector.scope.${connector.source}`) }) : t(`connector.scope.${connector.source}`)}</span>
                  <span aria-hidden="true">›</span>
                </button>
              ))}
            </div>
          </section>
        ))}
      </main>

      {selected ? (
        <div className="tc-modal-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) setSelectedConfigKey(null); }}>
          <section aria-label={t("connector.configure", { name: selected.name })} aria-modal="true" className="tc-modal tc-connector-modal" role="dialog" onKeyDown={(event) => { if (event.key === "Escape") setSelectedConfigKey(null); }}>
            <button aria-label={t("connector.closeDetails")} className="tc-modal__close" onClick={() => setSelectedConfigKey(null)} type="button">×</button>
            <h2>{selected.name}</h2>
            <dl className="tc-connector-facts">
              <div><dt>{t("connector.stateLabel")}</dt><dd role="status" aria-live="polite">{selectedBusy || selected.state === "connecting" ? <span className="tc-spinner tc-connector-spinner" aria-hidden="true" /> : <span className={statusClass(selected)} aria-hidden="true">●</span>} {statusLabel(selected, selectedBusy, t)}</dd></div>
              <div><dt>{t("connector.scopeLabel")}</dt><dd>{t(`connector.scope.${selected.source}`)}</dd></div>
              <div>
                <dt>{t("connector.configFile")}</dt>
                <dd>{configurationPath(selected) ? <ConnectorConfigPathLink onClick={() => send(vscodeApi, "openConnectorConfig", { configKey: selected.configKey })} path={configurationPath(selected)!} testId="connector-config-path" /> : <span className="tc-muted">{t("connector.configUnavailable")}</span>}</dd>
              </div>
              <div><dt>{t("connector.connection")}</dt><dd><code>{t(`term.transport.${selected.transport}`)}</code></dd></div>
              {selected.transport === "http" ? (
                <>
                  <div><dt>{t("connector.remoteUrl")}</dt><dd><code>{selected.url ?? "—"}</code></dd></div>
                  <div><dt>{t("connector.authentication")}</dt><dd>{authenticationLabel(selected, t)}</dd></div>
                </>
              ) : <div><dt>{t("connector.localCommand")}</dt><dd><code>{selected.command ?? "—"}</code></dd></div>}
            </dl>
            {selectedFeedback ? <div className="tc-banner tc-connector-feedback" role="status" aria-live="polite">{selectedFeedback}</div> : null}
            {selected.overridden ? <div className="tc-banner tc-banner--warning">{t("connector.overriddenHint")}</div> : null}

            {selected.transport === "http" && (selected.oauthConfigured || selected.auth === "oauth") ? (
              <div className="tc-connector-inline-actions">
                <button className="tc-button tc-button--secondary" data-testid="connector-login" disabled={selected.overridden || selected.state === "awaiting_project_trust" || loginBusy} onClick={() => { const requestId = send(vscodeApi, "loginConnector", { name: selected.name, configKey: selected.configKey }); setLoginRequest({ configKey: selected.configKey, requestId }); }} type="button">{t(loginBusy ? "connector.authorizing" : "connector.login")}</button>
                {loginBusy ? <button className="tc-button tc-button--secondary" data-testid="connector-cancel-login" disabled={selected.overridden || selected.state === "awaiting_project_trust"} onClick={() => { send(vscodeApi, "cancelLoginConnector", { name: selected.name, configKey: selected.configKey }); setLoginRequest(null); }} type="button">{t("common.cancel")}</button> : null}
                <button className="tc-button tc-button--secondary" disabled={selected.overridden || selected.state === "awaiting_project_trust"} onClick={() => send(vscodeApi, "logoutConnector", { name: selected.name, configKey: selected.configKey })} type="button">{t("connector.logout")}</button>
              </div>
            ) : null}

            <section className="tc-connector-flat-section">
              <div className="tc-connector-tools-heading">
                <h3>{toolsAvailable ? t("connector.toolsCount", { count: tools.length, enabled: tools.filter(tool => tool.enabled).length }) : t("settings.tools")}</h3>
                <p className="tc-connector-detail-section__description">{t(toolsAvailable ? "connector.toolsHint" : selectedBusy || selected.state === "connecting" ? "connector.afterReconnect" : selected.compatibilityError ? "connector.toolsUnavailable" : selected.state === "connected" ? "connector.toolsLoading" : "connector.untilConnected")}</p>
              </div>
              <div className="tc-connector-tools">
                {(toolsAvailable ? tools : []).map((tool) => {
                  const saving = sourceToggleBusy(selected.configKey);
                  const receipt = state.connectorToolToggles?.[toolToggleKey(selected.configKey, tool.rawName)];
                  const awaitingVerification = receipt !== undefined && (receipt.configSaved === undefined
                    || (receipt.configSaved === true && receipt.runtimeApplied === false));
                  const unavailable = selected.overridden || !canToggleTools || awaitingVerification;
                  return (
                    <button aria-busy={saving} aria-checked={tool.enabled} aria-disabled={unavailable || saving} aria-label={tool.label} className="tc-connector-tool" data-testid={`connector-tool-${tool.rawName}`} disabled={unavailable} key={tool.modelName} onClick={() => toggleTool(tool)} role="switch" type="button">
                      <span>{tool.label}</span>
                      {saving ? <span className="tc-spinner tc-connector-spinner" aria-hidden="true" /> : <span className={`tc-connector-tool__indicator ${tool.enabled ? "tc-connector-tool__indicator--enabled" : ""}`} aria-hidden="true" />}
                    </button>
                  );
                })}
              </div>
              {toolToggleFeedback ? (
                <div className="tc-connector-tool-feedback" role="status" aria-live="polite">
                  <span>{toolToggleFeedback}</span>
                  {configurationPath(selected) ? <ConnectorConfigPathLink onClick={() => send(vscodeApi, "openConnectorConfig", { configKey: selected.configKey })} path={configurationPath(selected)!} /> : null}
                  {retryableToolToggle && lastToolToggle ? <button className="tc-button tc-button--secondary" disabled={!canRetryToolToggle} onClick={() => requestToolToggle(lastToolToggle.rawName, lastToolToggle.enabled, true)} type="button">{t("common.retry")}</button> : null}
                  {retryableToolToggle && !canRetryToolToggle ? <span>{t("connector.retryHint")}</span> : null}
                </div>
              ) : null}
            </section>

            <footer className="tc-modal__footer tc-connector-modal__footer">
              <button aria-busy={selectedBusy} className="tc-button tc-button--secondary" data-testid="connector-reload" disabled={selected.overridden || sourceToggleBusy(selected.configKey) || selectedBusy || selected.state === "needs_authorization" || selected.state === "awaiting_project_trust" || Boolean(selected.compatibilityError)} onClick={() => reload(selected)} type="button">{selectedBusy ? <><span className="tc-spinner tc-connector-spinner" aria-hidden="true" /> {t("connector.state.connecting")}</> : t("connector.reload")}</button>
              <div className="tc-connector-modal__footer-actions">
                <button className="tc-button tc-button--danger" data-testid="connector-remove" onClick={() => { send(vscodeApi, "removeConnector", { name: selected.name, configKey: selected.configKey }); setSelectedConfigKey(null); }} type="button">{t("connector.remove")}</button>
                <button className="tc-button tc-button--primary" data-testid="connector-detail-done" onClick={() => setSelectedConfigKey(null)} type="button">{t("connector.done")}</button>
              </div>
            </footer>
          </section>
        </div>
      ) : null}

      {showAdd ? (
        <div className="tc-modal-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) setShowAdd(false); }}>
          <section aria-label={t("connector.addTitle")} className="tc-modal tc-connector-modal" role="dialog">
            <button aria-label={t("common.close")} className="tc-modal__close" onClick={() => setShowAdd(false)} type="button">×</button>
            <h2>{t("connector.addTitle")}</h2>
            <div className="tc-connector-form-row"><span>{t("connector.name")}</span><input aria-label={t("connector.name")} data-testid="connector-name" value={name} onChange={(event) => setName(event.target.value)} /></div>
            <div className="tc-connector-form-row"><span>{t("connector.type")}</span><div className="tc-connector-radio-row">
              <label><input checked type="radio" onChange={() => {}} /> {t("term.mcp")}</label>
              <label className="tc-muted"><input disabled type="radio" /> {t("connector.soon", { type: t("term.cli") })}</label>
              <label className="tc-muted"><input disabled type="radio" /> {t("connector.soon", { type: t("term.a2a") })}</label>
            </div></div>
            <div className="tc-connector-form-row"><span>{t("connector.scopeLabel")}</span><div className="tc-connector-radio-row">
              <label><input checked={scope === "global"} data-testid="connector-scope-global" name="scope" onChange={() => setScope("global")} type="radio" /> {t("connector.scope.global")}</label>
              <label className={state.connectorConfigPaths?.workspace ? undefined : "tc-muted"}><input checked={scope === "workspace"} data-testid="connector-scope-workspace" disabled={!state.connectorConfigPaths?.workspace} name="scope" onChange={() => setScope("workspace")} type="radio" /> {t(state.connectorConfigPaths?.workspace ? "connector.scope.workspace" : "connector.workspaceUnavailable")}</label>
            </div></div>
            <div className="tc-connector-form-row"><span>{t("connector.configFile")}</span>{configurationPathForScope(state, scope) ? <ConnectorConfigPathLink onClick={() => send(vscodeApi, "openConnectorConfig", { scope })} path={configurationPathForScope(state, scope)!} testId="connector-add-config-path" /> : <span className="tc-muted">{t("connector.configUnavailable")}</span>}</div>
            <div className="tc-connector-form-row"><span>{t("connector.connection")}</span><div className="tc-connector-radio-row">
              <label><input checked={transport === "stdio"} data-testid="connector-transport-stdio" name="transport" onChange={() => setTransport("stdio")} type="radio" /> {t("term.transport.stdio")}</label>
              <label><input checked={transport === "http"} data-testid="connector-transport-http" name="transport" onChange={() => setTransport("http")} type="radio" /> {t("term.http")}</label>
            </div></div>
            {transport === "http" && authMode === "oauth" ? <div className="tc-connector-form-row"><span>{t("term.field.oauthClientId")}</span><div><input aria-label={t("term.field.oauthClientId")} placeholder={t("connector.optional")} value={clientId} onChange={(event) => setClientId(event.target.value)} /><small>{t("connector.registrationHint")}</small></div></div> : null}
            {transport === "http" ? <>
              <div className="tc-connector-form-row"><span>{t("connector.remoteUrl")}</span><input aria-label={t("term.field.url")} data-testid="connector-url" placeholder="https://example.com/mcp" value={url} onChange={(event) => setUrl(event.target.value)} /></div>
              <div className="tc-connector-form-row"><span>{t("connector.authentication")}</span><select aria-label={t("connector.authentication")} data-testid="connector-auth" value={authMode} onChange={(event) => setAuthMode(event.target.value as "oauth" | "bearer" | "none")}>
                <option value="oauth">{t("term.oauth2")}</option><option value="bearer">{t("term.field.bearerToken")}</option><option value="none">{t("connector.auth.none")}</option>
              </select></div>
              {authMode === "bearer" ? <div className="tc-connector-form-row"><span>{t("term.field.bearerToken")}</span><input aria-label={t("term.field.bearerToken")} type="password" placeholder={t("connector.storedLocally")} value={bearerToken} onChange={(event) => setBearerToken(event.target.value)} /></div> : null}
              <div className="tc-connector-form-row"><span>{t("connector.headers")}</span><div><textarea aria-label={t("connector.headers")} rows={3} placeholder="Header: value" value={customHeaders} onChange={(event) => setCustomHeaders(event.target.value)} /><small>{t("connector.headersHint")}</small></div></div>
              <p className="tc-connector-form-help">{t("connector.addHint")}</p>
            </> : <>
              <div className="tc-connector-form-row"><span>{t("connector.localCommand")}</span><input aria-label={t("connector.command")} data-testid="connector-command" placeholder="npx" value={command} onChange={(event) => setCommand(event.target.value)} /></div>
              <div className="tc-connector-form-row"><span>{t("connector.arguments")}</span><input aria-label={t("connector.argsAria")} data-testid="connector-args" placeholder="-y @playwright/mcp" value={args} onChange={(event) => setArgs(event.target.value)} /></div>
              <div className="tc-connector-form-row"><span>{t("connector.environment")}</span><div><textarea aria-label={t("connector.environment")} rows={3} placeholder="KEY=value" value={envText} onChange={(event) => setEnvText(event.target.value)} /><small>{t("connector.environmentHint")}</small></div></div>
            </>}
            {formError ? <div className="tc-banner tc-banner--warning">{typeof formError === "string" ? formError : t(formError.key)}</div> : null}
            <footer className="tc-modal__footer">
              <button className="tc-button tc-button--secondary" onClick={() => { setSubmissionId(null); setIsSubmitting(false); submitLock.current = false; setShowAdd(false); }} type="button">{t("common.cancel")}</button>
              <button className="tc-button tc-button--primary" data-testid="connector-add-submit" disabled={isSubmitting} onClick={submitAdd} type="button">{t(isSubmitting ? "settings.language.saving" : scope === "workspace" && state.connectorProject?.trusted === false ? "connector.addAndTrust" : "connector.add")}</button>
            </footer>
          </section>
        </div>
      ) : null}
    </SettingsShell>
  );
}
