import { useEffect, useMemo, useState } from "react";

import type {
  ConnectorInput,
  ConnectorToolFilter,
  ConnectorToolView,
  ConnectorView,
} from "../../../src/shared/connectorsProtocol";
import type {
  SettingsIntent,
  SettingsStateSnapshot,
  VsCodeApiLike,
} from "../../../src/shared/settingsProtocol";

function send(
  vscodeApi: VsCodeApiLike<SettingsIntent>,
  type: SettingsIntent["type"],
  data?: unknown,
): void {
  vscodeApi.postMessage({
    messageId: `connector-${type}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
    type,
    ...(data === undefined ? {} : { data }),
  } as SettingsIntent);
}

function statusLabel(connector: ConnectorView): string {
  switch (connector.state) {
    case "connected":
      return "Connected";
    case "connecting":
      return "Connecting";
    case "needs_confirmation":
      return "Needs confirmation";
    case "needs_authorization":
      return "Authorization required";
    case "blocked":
      return "Blocked";
    case "disconnected":
      return "Disconnected";
    default:
      return "Failed";
  }
}

function statusClass(connector: ConnectorView): string {
  return `tc-connector-status tc-connector-status--${connector.state}`;
}

function configurationPath(connector: ConnectorView): string | null {
  return connector.configPath ?? null;
}

function configurationPathForScope(
  state: SettingsStateSnapshot,
  scope: "user" | "workspace",
): string | null {
  const configured = scope === "user"
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

export function ConnectorsSettingsView({
  state,
  vscodeApi,
}: {
  state: SettingsStateSnapshot;
  vscodeApi: VsCodeApiLike<SettingsIntent>;
}) {
  const connectors = state.connectors ?? [];
  const [selected, setSelected] = useState<ConnectorView | null>(null);
  const [tools, setTools] = useState<ConnectorToolView[]>([]);
  const [showAdd, setShowAdd] = useState(false);
  const [transport, setTransport] = useState<"stdio" | "http">("stdio");
  const [url, setUrl] = useState("");
  const [command, setCommand] = useState("");
  const [args, setArgs] = useState("");
  const [name, setName] = useState("");
  const [authMode, setAuthMode] = useState<"oauth" | "bearer" | "none">("oauth");
  const [bearerToken, setBearerToken] = useState("");
  const [customHeaders, setCustomHeaders] = useState("");
  const [formError, setFormError] = useState<string | null>(null);
  const [envText, setEnvText] = useState("");
  // `user` remains the stable wire value for the global configuration file.
  const [scope, setScope] = useState<"workspace" | "user">("user");
  const [clientId, setClientId] = useState("");
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [busyAction, setBusyAction] = useState<string | null>(null);

  useEffect(() => {
    if (state.selectedConnector === selected?.name) {
      setTools(state.connectorTools ?? []);
    }
  }, [selected?.name, state.connectorTools, state.selectedConnector]);

  useEffect(() => {
    if (!isSubmitting || !state.status || state.status === "Authorizing connector…") {
      return;
    }
    setIsSubmitting(false);
    if (state.status.includes("connected") || state.status === "Connector saved.") {
      setShowAdd(false);
    }
  }, [isSubmitting, state.status]);

  useEffect(() => {
    if (!selected) {
      return;
    }
    const fresh = connectors.find((connector) => connector.name === selected.name);
    if (fresh) {
      setSelected(fresh);
    }
  }, [connectors, selected?.name]);

  useEffect(() => {
    if (state.status && state.status !== "Authorizing connector…") {
      setBusyAction(null);
    }
  }, [state.status]);

  const groups = useMemo(() => {
    const order: ConnectorView["state"][] = [
      "connected",
      "connecting",
      "needs_confirmation",
      "needs_authorization",
      "failed",
      "disconnected",
      "blocked",
    ];
    return order
      .map((group) => ({
        label:
          group === "needs_confirmation"
            ? "Needs confirmation"
            : group === "needs_authorization"
              ? "Authorization required"
              : group,
        items: connectors.filter((connector) => connector.state === group),
      }))
      .filter((group) => group.items.length > 0);
  }, [connectors]);

  function openDetail(connector: ConnectorView): void {
    setSelected(connector);
    setTools([]);
    send(vscodeApi, "listConnectorTools", { name: connector.name });
  }

  function openAdd(): void {
    setScope("user");
    setFormError(null);
    setIsSubmitting(false);
    setShowAdd(true);
  }

  function toggleTool(tool: ConnectorToolView): void {
    if (!selected) {
      return;
    }
    const next = tools.map((entry) =>
      entry.modelName === tool.modelName ? { ...entry, enabled: !entry.enabled } : entry,
    );
    setTools(next);
    const currentToolNames = new Set(tools.map((entry) => entry.rawName));
    const filter: ConnectorToolFilter = {
      include: Array.from(
        new Set([
          ...(selected.toolFilter?.include ?? []),
          ...next.filter((entry) => entry.enabled).map((entry) => entry.rawName),
        ]),
      ),
      exclude: Array.from(
        new Set([
          ...(selected.toolFilter?.exclude ?? []).filter((entry) => !currentToolNames.has(entry)),
          ...next.filter((entry) => !entry.enabled).map((entry) => entry.rawName),
        ]),
      ),
    };
    send(vscodeApi, "setConnectorToolFilter", { name: selected.name, filter });
  }

  function submitAdd(): void {
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
    send(vscodeApi, "addConnector", { connector: input });
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
          <button className="tc-button tc-button--secondary" onClick={openAdd} type="button">+ Add Connector</button>
        </header>
        {state.error ? <div className="tc-banner tc-banner--warning">{state.error}</div> : null}
        {state.status ? <div className="tc-banner">{state.status}</div> : null}
        {groups.length === 0 ? (
          <section className="tc-empty-state">
            <h2>No connectors configured</h2>
            <p>Add a Global connector to make external tools available in every workspace.</p>
          </section>
        ) : groups.map((group) => (
          <section className="tc-settings-group" key={group.label}>
            <h2 className="tc-settings-group__title">{group.label}</h2>
            <div className="tc-connector-list">
              {group.items.map((connector) => (
                <button className="tc-connector-card" key={connector.name} onClick={() => openDetail(connector)} type="button">
                  <span className={statusClass(connector)} aria-hidden="true">●</span>
                  <span className="tc-connector-card__body">
                    <strong>{connector.name}</strong>
                    <span>{statusLabel(connector)} · {connector.toolCount} tools · {connector.transport}</span>
                  </span>
                  <span className="tc-connector-card__source">{connector.source}</span>
                  <span aria-hidden="true">›</span>
                </button>
              ))}
            </div>
          </section>
        ))}
      </main>

      {selected ? (
        <div className="tc-modal-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) setSelected(null); }}>
          <section aria-label={`Configure ${selected.name}`} className="tc-modal tc-connector-modal" role="dialog">
            <button className="tc-modal__close" onClick={() => setSelected(null)} type="button">×</button>
            <h2>{selected.name}</h2>
            <dl className="tc-connector-facts">
              <div><dt>State</dt><dd><span className={statusClass(selected)} aria-hidden="true">●</span> {statusLabel(selected)}</dd></div>
              <div><dt>Scope</dt><dd>{selected.source}</dd></div>
              <div>
                <dt>Config file</dt>
                <dd>{configurationPath(selected) ? <button className="tc-connector-config-link" onClick={() => send(vscodeApi, "openConnectorConfig", { name: selected.name })} type="button"><code>{configurationPath(selected)}</code></button> : <span className="tc-muted">Configuration file unavailable</span>}</dd>
              </div>
              <div><dt>Connection</dt><dd><code>{selected.transport}</code></dd></div>
              {selected.transport === "http" ? (
                <>
                  <div><dt>Remote URL</dt><dd><code>{selected.url ?? "—"}</code></dd></div>
                  <div><dt>Authentication</dt><dd>{authenticationLabel(selected)}</dd></div>
                </>
              ) : <div><dt>Local command</dt><dd><code>{selected.command ?? "—"}</code></dd></div>}
            </dl>

            {selected.transport === "http" && (selected.oauthConfigured || selected.auth === "oauth") ? (
              <div className="tc-connector-inline-actions">
                <button className="tc-button tc-button--secondary" disabled={busyAction === "login"} onClick={() => { setBusyAction("login"); send(vscodeApi, "loginConnector", { name: selected.name }); }} type="button">{busyAction === "login" ? "Authorizing…" : "Login / Re-login"}</button>
                {busyAction === "login" ? <button className="tc-button tc-button--secondary" onClick={() => { send(vscodeApi, "cancelLoginConnector", { name: selected.name }); setBusyAction(null); }} type="button">Cancel</button> : null}
                <button className="tc-button tc-button--secondary" onClick={() => send(vscodeApi, "logoutConnector", { name: selected.name })} type="button">Logout</button>
              </div>
            ) : null}

            <section className="tc-connector-flat-section">
              <div className="tc-connector-tools-heading">
                <h3>Tools ({tools.length})</h3>
                <p className="tc-connector-detail-section__description">Green = available · outline = hidden</p>
              </div>
              <div className="tc-connector-tools">
                {tools.map((tool) => (
                  <button aria-label={`${tool.label}: ${tool.enabled ? "enabled" : "disabled"}`} className="tc-connector-tool" key={tool.modelName} onClick={() => toggleTool(tool)} type="button">
                    <span>{tool.label}</span>
                    <span className={`tc-connector-tool__indicator ${tool.enabled ? "tc-connector-tool__indicator--enabled" : ""}`} aria-hidden="true" />
                  </button>
                ))}
              </div>
            </section>

            <footer className="tc-modal__footer tc-connector-modal__footer">
              <button className="tc-button tc-button--secondary" onClick={() => send(vscodeApi, "reloadConnector", { name: selected.name })} type="button">↻ Reload</button>
              <div className="tc-connector-modal__footer-actions">
                {selected.state === "needs_confirmation" || selected.state === "blocked" ? <button className="tc-button" onClick={() => send(vscodeApi, "trustConnector", { name: selected.name })} type="button">Trust</button> : null}
                {selected.state === "needs_confirmation" ? <button className="tc-button tc-button--secondary" onClick={() => send(vscodeApi, "denyConnector", { name: selected.name })} type="button">Deny</button> : null}
                <button className="tc-button tc-button--danger" onClick={() => { send(vscodeApi, "removeConnector", { name: selected.name }); setSelected(null); }} type="button">Remove</button>
                <button className="tc-button tc-button--primary" onClick={() => setSelected(null)} type="button">Done</button>
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
            <div className="tc-connector-form-row"><span>Name</span><input aria-label="Name" value={name} onChange={(event) => setName(event.target.value)} /></div>
            <div className="tc-connector-form-row"><span>Type</span><div className="tc-connector-radio-row"><label><input checked type="radio" onChange={() => {}} /> MCP</label><label className="tc-muted"><input disabled type="radio" /> CLI (soon)</label><label className="tc-muted"><input disabled type="radio" /> A2A (soon)</label></div></div>
            <div className="tc-connector-form-row"><span>Scope</span><div className="tc-connector-radio-row"><label><input checked={scope === "user"} name="scope" onChange={() => setScope("user")} type="radio" /> Global</label><label><input checked={scope === "workspace"} name="scope" onChange={() => setScope("workspace")} type="radio" /> Workspace</label></div></div>
            <div className="tc-connector-form-row"><span>Config file</span>{configurationPathForScope(state, scope) ? <button className="tc-connector-config-link" onClick={() => send(vscodeApi, "openConnectorConfig", { scope })} type="button"><code>{configurationPathForScope(state, scope)}</code></button> : <span className="tc-muted">Configuration file unavailable</span>}</div>
            <div className="tc-connector-form-row"><span>Connection</span><div className="tc-connector-radio-row"><label><input checked={transport === "stdio"} name="transport" onChange={() => setTransport("stdio")} type="radio" /> stdio</label><label><input checked={transport === "http"} name="transport" onChange={() => setTransport("http")} type="radio" /> HTTP</label></div></div>
            {transport === "http" && authMode === "oauth" ? <div className="tc-connector-form-row"><span>OAuth client ID</span><div><input aria-label="OAuth client ID" placeholder="Optional" value={clientId} onChange={(event) => setClientId(event.target.value)} /><small>Dynamic registration is used when empty.</small></div></div> : null}
            {transport === "http" ? <><div className="tc-connector-form-row"><span>Remote URL</span><input aria-label="URL" placeholder="https://example.com/mcp" value={url} onChange={(event) => setUrl(event.target.value)} /></div><div className="tc-connector-form-row"><span>Authentication</span><select aria-label="Authentication" value={authMode} onChange={(event) => setAuthMode(event.target.value as "oauth" | "bearer" | "none")}><option value="oauth">OAuth 2.0</option><option value="bearer">Bearer token</option><option value="none">None</option></select></div>{authMode === "bearer" ? <div className="tc-connector-form-row"><span>Bearer token</span><input aria-label="Bearer token" type="password" placeholder="Stored locally" value={bearerToken} onChange={(event) => setBearerToken(event.target.value)} /></div> : null}<div className="tc-connector-form-row"><span>Custom headers</span><div><textarea aria-label="Custom headers" rows={3} placeholder="Header: value" value={customHeaders} onChange={(event) => setCustomHeaders(event.target.value)} /><small>One header per line.</small></div></div><p className="tc-connector-form-help">Add saves the configuration and starts OAuth authorization.</p></> : <><div className="tc-connector-form-row"><span>Local command</span><input aria-label="Command" placeholder="npx" value={command} onChange={(event) => setCommand(event.target.value)} /></div><div className="tc-connector-form-row"><span>Arguments</span><input aria-label="Args" placeholder="-y @playwright/mcp" value={args} onChange={(event) => setArgs(event.target.value)} /></div><div className="tc-connector-form-row"><span>Environment</span><div><textarea aria-label="Environment" rows={3} placeholder="KEY=value" value={envText} onChange={(event) => setEnvText(event.target.value)} /><small>One variable per line.</small></div></div></>}
            {formError ? <div className="tc-banner tc-banner--warning">{formError}</div> : null}
            <footer className="tc-modal__footer"><button className="tc-button tc-button--secondary" onClick={() => { if (isSubmitting) send(vscodeApi, "cancelLoginConnector", { name: name.trim() }); setIsSubmitting(false); setShowAdd(false); }} type="button">Cancel</button><button className="tc-button tc-button--primary" disabled={isSubmitting} onClick={submitAdd} type="button">{isSubmitting ? "Connecting…" : "Add"}</button></footer>
          </section>
        </div>
      ) : null}
    </div>
  );
}
