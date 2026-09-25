import { useEffect, useMemo, useRef, useState } from "react";

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

function statusLabel(connector: ConnectorView, busy = false): string {
  if (busy || connector.state === "connecting") {
    const count = connector.state === "connecting" && connector.attempt && connector.recovery
      ? ` ${connector.attempt}/${connector.recovery.maxAttempts}` : "";
    return `Reconnecting…${count}`;
  }
  switch (connector.state) {
    case "connected":
      return "Connected";
    case "pending":
      return "Pending";
    case "awaiting_project_trust":
      return "Awaiting project trust";
    case "needs_authorization":
      return "Authorization required";
    case "disconnected":
      return "Disconnected";
    default:
      return "Failed";
  }
}

function statusClass(connector: ConnectorView): string {
  return `tc-connector-status tc-connector-status--${connector.state}`;
}

function reloadFeedback(connector: ConnectorView, receipt?: SettingsConnectorReloadReceipt): string | null {
  if (connector.compatibilityError) return connector.compatibilityError;
  if (receipt?.phase === "unknown" || receipt?.reason === "superseded" || receipt?.reason === "removed") return receipt.message ?? "Unable to confirm reconnection.";
  if (receipt?.phase === "failed" || connector.state === "failed") {
    const attempts = connector.attempt ? ` after ${connector.attempt} attempt${connector.attempt === 1 ? "" : "s"}` : "";
    return `Reconnect failed${attempts}. ${receipt?.message ?? connector.error ?? "Check the connection settings."}`;
  }
  return receipt?.phase === "succeeded" && connector.state === "connected" ? "Connector reconnected." : connector.error ?? null;
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

function authenticationLabel(connector: ConnectorView): string {
  if (connector.auth === "oauth" || connector.oauthConfigured) {
    return "OAuth 2.0";
  }
  if (connector.auth === "bearer") {
    return "Bearer token";
  }
  return "None";
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
  const [formError, setFormError] = useState<string | null>(null);
  const [envText, setEnvText] = useState("");
  const [scope, setScope] = useState<"workspace" | "global">("global");
  const [submissionId, setSubmissionId] = useState<string | null>(null);
  const [clientId, setClientId] = useState("");
  const [isSubmitting, setIsSubmitting] = useState(false);
  const submitLock = useRef(false);
  const trustLock = useRef<string | null>(null);
  const [busyAction, setBusyAction] = useState<string | null>(null);

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
  const selectedFeedback = selected ? reloadFeedback(selected, state.connectorReloads?.[selected.configKey]) : null;
  const canToggleTools = state.capabilities.connectorCapabilities?.toggle === true;
  const lastToolToggle = lastToolToggleKey && selected
    ? state.connectorToolToggles?.[lastToolToggleKey]
    : undefined;
  const toolToggleFeedback = lastToolToggle && lastToolToggle.configKey === selected?.configKey
    && (!lastToolToggle.configSaved || !lastToolToggle.runtimeApplied)
    ? lastToolToggle.error ?? "Unable to update this tool."
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
    setFormError(receipt.error ?? "Unable to add connector.");
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
    if (state.status && state.status !== "Authorizing connector…") {
      setBusyAction(null);
    }
  }, [state.status]);

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
        label:
          group === "awaiting_project_trust"
            ? "Awaiting project trust"
            : group === "needs_authorization"
              ? "Authorization required"
              : group,
        items: connectors.filter((connector) => connector.state === group),
        state: group,
      }))
      .filter((group) => group.items.length > 0);
  }, [connectors]);

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
      setFormError("Connector name is required.");
      return;
    }
    if (transport === "http" && !/^https?:\/\//i.test(url.trim())) {
      setFormError("Enter an HTTP(S) MCP URL.");
      return;
    }
    if (transport === "stdio" && !command.trim()) {
      setFormError("Enter a command for a stdio connector.");
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
        setFormError("Enter a bearer token.");
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
      setFormError("Update Tomcat Serve to trust this project before adding a connector.");
      return;
    }
    const requestId = send(vscodeApi, "addConnector", { connector: input, ...(trustProject ? { trustProject: true } : {}) });
    submitLock.current = true;
    setSubmissionId(requestId);
    setIsSubmitting(true);
    setFormError(null);
  }

  return (
    <div className="tc-settings-shell">
      <aside className="tc-settings-shell__nav">
        <div className="tc-settings-shell__brand">Tomcat Settings</div>
        <button className="tc-settings-nav__item" onClick={() => send(vscodeApi, "settings.ready", { route: "models" })} type="button">Models</button>
        <button className="tc-settings-nav__item" disabled type="button">Sessions</button>
        <button className="tc-settings-nav__item" disabled type="button">Tools</button>
        <button className="tc-settings-nav__item tc-settings-nav__item--active" type="button">Connectors</button>
        <div className="tc-settings-shell__version">
          <div>Extension {state.extensionVersion ?? "unknown"}</div>
          <div>Serve {state.serverVersion ?? "unknown"}</div>
        </div>
      </aside>
      <main className="tc-settings-shell__content">
        <header className="tc-settings-shell__header">
          <div>
            <h1>Connectors</h1>
            <p>Connect MCP services. Their tools are available only when Tomcat needs them.</p>
          </div>
          <button className="tc-button tc-button--secondary" data-testid="connector-add-open" onClick={openAdd} type="button">+ Add Connector</button>
        </header>
        {state.error ? <div className="tc-banner tc-banner--warning">{state.error}</div> : null}
        {state.status ? <div className="tc-banner">{state.status}</div> : null}
        <div className="tc-banner tc-settings-restart-hint" role="status">
          Changes to the project resource directory take effect after restarting Tomcat.
        </div>
        {groups.length === 0 ? (
          <section className="tc-empty-state">
            <h2>No connectors configured</h2>
            <p>Add a Global connector to make external tools available in every workspace.</p>
          </section>
        ) : groups.map((group) => (
          <section className="tc-settings-group" key={group.label}>
            <div className="tc-settings-group__heading">
              <h2 className="tc-settings-group__title">{group.label}</h2>
              {group.state === "awaiting_project_trust" && state.connectorProject?.trusted === false ? (
                <button className="tc-button tc-button--secondary" data-testid="connector-trust-project" disabled={state.connectorTrustPending || !state.capabilities.connectorCapabilities?.trustProject} onClick={requestProjectTrust} type="button">{state.connectorTrustPending ? "Trusting…" : "Trust project"}</button>
              ) : null}
            </div>
            {group.state === "awaiting_project_trust" && state.connectorProject ? <div className="tc-settings-group__project-path" title={state.connectorProject.root}>{state.connectorProject.root}</div> : null}
            <div className="tc-connector-list">
              {group.items.map((connector) => (
                <button className="tc-connector-card" data-testid={`connector-card-${connector.name}`} key={connector.configKey} onClick={() => openDetail(connector)} type="button">
                  {reloadBusy(connector) || connector.state === "connecting" ? <span className="tc-spinner tc-connector-spinner" aria-hidden="true" /> : <span className={statusClass(connector)} aria-hidden="true">●</span>}
                  <span className="tc-connector-card__body">
                    <strong>{connector.name}</strong>
                    <span role="status" aria-live={selected?.configKey === connector.configKey ? "off" : "polite"}>{statusLabel(connector, reloadBusy(connector))} · {connector.overridden ? "Overridden by workspace" : `${connector.state === "connected" && !reloadBusy(connector) ? `${connector.toolCount} enabled tools · ` : ""}${connector.transport}`}</span>
                    {reloadFeedback(connector, state.connectorReloads?.[connector.configKey]) ? <span className="tc-connector-feedback">{reloadFeedback(connector, state.connectorReloads?.[connector.configKey])}</span> : null}
                  </span>
                  <span className="tc-connector-card__source">{connector.source}{connector.overridden ? " · overridden" : ""}</span>
                  <span aria-hidden="true">›</span>
                </button>
              ))}
            </div>
          </section>
        ))}
      </main>

      {selected ? (
        <div className="tc-modal-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) setSelectedConfigKey(null); }}>
          <section aria-label={`Configure ${selected.name}`} aria-modal="true" className="tc-modal tc-connector-modal" role="dialog" onKeyDown={(event) => { if (event.key === "Escape") setSelectedConfigKey(null); }}>
            <button aria-label="Close connector details" className="tc-modal__close" onClick={() => setSelectedConfigKey(null)} type="button">×</button>
            <h2>{selected.name}</h2>
            <dl className="tc-connector-facts">
              <div><dt>State</dt><dd role="status" aria-live="polite">{selectedBusy || selected.state === "connecting" ? <span className="tc-spinner tc-connector-spinner" aria-hidden="true" /> : <span className={statusClass(selected)} aria-hidden="true">●</span>} {statusLabel(selected, selectedBusy)}</dd></div>
              <div><dt>Scope</dt><dd>{selected.source}</dd></div>
              <div>
                <dt>Config file</dt>
                <dd>{configurationPath(selected) ? <ConnectorConfigPathLink onClick={() => send(vscodeApi, "openConnectorConfig", { configKey: selected.configKey })} path={configurationPath(selected)!} testId="connector-config-path" /> : <span className="tc-muted">Configuration file unavailable</span>}</dd>
              </div>
              <div><dt>Connection</dt><dd><code>{selected.transport}</code></dd></div>
              {selected.transport === "http" ? (
                <>
                  <div><dt>Remote URL</dt><dd><code>{selected.url ?? "—"}</code></dd></div>
                  <div><dt>Authentication</dt><dd>{authenticationLabel(selected)}</dd></div>
                </>
              ) : <div><dt>Local command</dt><dd><code>{selected.command ?? "—"}</code></dd></div>}
            </dl>
            {selectedFeedback ? <div className="tc-banner tc-connector-feedback" role="status" aria-live="polite">{selectedFeedback}</div> : null}
            {selected.overridden ? <div className="tc-banner tc-banner--warning">This Global connector is overridden by the same-named Workspace connector. Remove the Workspace connector to make it active again.</div> : null}

            {selected.transport === "http" && (selected.oauthConfigured || selected.auth === "oauth") ? (
              <div className="tc-connector-inline-actions">
                <button className="tc-button tc-button--secondary" data-testid="connector-login" disabled={selected.overridden || selected.state === "awaiting_project_trust" || busyAction === "login"} onClick={() => { setBusyAction("login"); send(vscodeApi, "loginConnector", { name: selected.name, configKey: selected.configKey }); }} type="button">{busyAction === "login" ? "Authorizing…" : "Login / Re-login"}</button>
                {busyAction === "login" ? <button className="tc-button tc-button--secondary" data-testid="connector-cancel-login" disabled={selected.overridden || selected.state === "awaiting_project_trust"} onClick={() => { send(vscodeApi, "cancelLoginConnector", { name: selected.name, configKey: selected.configKey }); setBusyAction(null); }} type="button">Cancel</button> : null}
                <button className="tc-button tc-button--secondary" disabled={selected.overridden || selected.state === "awaiting_project_trust"} onClick={() => send(vscodeApi, "logoutConnector", { name: selected.name, configKey: selected.configKey })} type="button">Logout</button>
              </div>
            ) : null}

            <section className="tc-connector-flat-section">
              <div className="tc-connector-tools-heading">
                <h3>{toolsAvailable ? `Tools (${tools.length}) · ${tools.filter((tool) => tool.enabled).length} enabled` : "Tools"}</h3>
                <p className="tc-connector-detail-section__description">{toolsAvailable ? "Green = enabled · outline = disabled" : selectedBusy || selected.state === "connecting" ? "Available after reconnection" : selected.compatibilityError ? "Tools unavailable" : selected.state === "connected" ? "Loading tools…" : "Tools unavailable until connected"}</p>
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
                  {retryableToolToggle && lastToolToggle ? <button className="tc-button tc-button--secondary" disabled={!canRetryToolToggle} onClick={() => requestToolToggle(lastToolToggle.rawName, lastToolToggle.enabled, true)} type="button">Retry</button> : null}
                  {retryableToolToggle && !canRetryToolToggle ? <span>Retry is available after this connector reconnects.</span> : null}
                </div>
              ) : null}
            </section>

            <footer className="tc-modal__footer tc-connector-modal__footer">
              <button aria-busy={selectedBusy} className="tc-button tc-button--secondary" data-testid="connector-reload" disabled={selected.overridden || sourceToggleBusy(selected.configKey) || selectedBusy || selected.state === "needs_authorization" || selected.state === "awaiting_project_trust" || Boolean(selected.compatibilityError)} onClick={() => reload(selected)} type="button">{selectedBusy ? <><span className="tc-spinner tc-connector-spinner" aria-hidden="true" /> Reconnecting…</> : "↻ Reload"}</button>
              <div className="tc-connector-modal__footer-actions">
                <button className="tc-button tc-button--danger" data-testid="connector-remove" onClick={() => { send(vscodeApi, "removeConnector", { name: selected.name, configKey: selected.configKey }); setSelectedConfigKey(null); }} type="button">Remove</button>
                <button className="tc-button tc-button--primary" data-testid="connector-detail-done" onClick={() => setSelectedConfigKey(null)} type="button">Done</button>
              </div>
            </footer>
          </section>
        </div>
      ) : null}

      {showAdd ? (
        <div className="tc-modal-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) setShowAdd(false); }}>
          <section aria-label="Add Connector" className="tc-modal tc-connector-modal" role="dialog">
            <button className="tc-modal__close" onClick={() => setShowAdd(false)} type="button">×</button>
            <h2>Add Connector</h2>
            <div className="tc-connector-form-row"><span>Name</span><input aria-label="Name" data-testid="connector-name" value={name} onChange={(event) => setName(event.target.value)} /></div>
            <div className="tc-connector-form-row"><span>Type</span><div className="tc-connector-radio-row"><label><input checked type="radio" onChange={() => {}} /> MCP</label><label className="tc-muted"><input disabled type="radio" /> CLI (soon)</label><label className="tc-muted"><input disabled type="radio" /> A2A (soon)</label></div></div>
            <div className="tc-connector-form-row"><span>Scope</span><div className="tc-connector-radio-row"><label><input checked={scope === "global"} data-testid="connector-scope-global" name="scope" onChange={() => setScope("global")} type="radio" /> Global</label><label className={state.connectorConfigPaths?.workspace ? undefined : "tc-muted"}><input checked={scope === "workspace"} data-testid="connector-scope-workspace" disabled={!state.connectorConfigPaths?.workspace} name="scope" onChange={() => setScope("workspace")} type="radio" /> Workspace{state.connectorConfigPaths?.workspace ? "" : " (open a project first)"}</label></div></div>
            <div className="tc-connector-form-row"><span>Config file</span>{configurationPathForScope(state, scope) ? <ConnectorConfigPathLink onClick={() => send(vscodeApi, "openConnectorConfig", { scope })} path={configurationPathForScope(state, scope)!} testId="connector-add-config-path" /> : <span className="tc-muted">Configuration file unavailable</span>}</div>
            <div className="tc-connector-form-row"><span>Connection</span><div className="tc-connector-radio-row"><label><input checked={transport === "stdio"} data-testid="connector-transport-stdio" name="transport" onChange={() => setTransport("stdio")} type="radio" /> stdio</label><label><input checked={transport === "http"} data-testid="connector-transport-http" name="transport" onChange={() => setTransport("http")} type="radio" /> HTTP</label></div></div>
            {transport === "http" && authMode === "oauth" ? <div className="tc-connector-form-row"><span>OAuth client ID</span><div><input aria-label="OAuth client ID" placeholder="Optional" value={clientId} onChange={(event) => setClientId(event.target.value)} /><small>Dynamic registration is used when empty.</small></div></div> : null}
            {transport === "http" ? <><div className="tc-connector-form-row"><span>Remote URL</span><input aria-label="URL" data-testid="connector-url" placeholder="https://example.com/mcp" value={url} onChange={(event) => setUrl(event.target.value)} /></div><div className="tc-connector-form-row"><span>Authentication</span><select aria-label="Authentication" data-testid="connector-auth" value={authMode} onChange={(event) => setAuthMode(event.target.value as "oauth" | "bearer" | "none")}><option value="oauth">OAuth 2.0</option><option value="bearer">Bearer token</option><option value="none">None</option></select></div>{authMode === "bearer" ? <div className="tc-connector-form-row"><span>Bearer token</span><input aria-label="Bearer token" type="password" placeholder="Stored locally" value={bearerToken} onChange={(event) => setBearerToken(event.target.value)} /></div> : null}<div className="tc-connector-form-row"><span>Custom headers</span><div><textarea aria-label="Custom headers" rows={3} placeholder="Header: value" value={customHeaders} onChange={(event) => setCustomHeaders(event.target.value)} /><small>One header per line.</small></div></div><p className="tc-connector-form-help">Add saves the configuration and starts a connection. OAuth authorization starts only when you choose Login.</p></> : <><div className="tc-connector-form-row"><span>Local command</span><input aria-label="Command" data-testid="connector-command" placeholder="npx" value={command} onChange={(event) => setCommand(event.target.value)} /></div><div className="tc-connector-form-row"><span>Arguments</span><input aria-label="Args" data-testid="connector-args" placeholder="-y @playwright/mcp" value={args} onChange={(event) => setArgs(event.target.value)} /></div><div className="tc-connector-form-row"><span>Environment</span><div><textarea aria-label="Environment" rows={3} placeholder="KEY=value" value={envText} onChange={(event) => setEnvText(event.target.value)} /><small>One variable per line.</small></div></div></>}
            {formError ? <div className="tc-banner tc-banner--warning">{formError}</div> : null}
            <footer className="tc-modal__footer"><button className="tc-button tc-button--secondary" onClick={() => { setSubmissionId(null); setIsSubmitting(false); submitLock.current = false; setShowAdd(false); }} type="button">Cancel</button><button className="tc-button tc-button--primary" data-testid="connector-add-submit" disabled={isSubmitting} onClick={submitAdd} type="button">{isSubmitting ? "Saving…" : scope === "workspace" && state.connectorProject?.trusted === false ? "Add and Trust" : "Add"}</button></footer>
          </section>
        </div>
      ) : null}
    </div>
  );
}
