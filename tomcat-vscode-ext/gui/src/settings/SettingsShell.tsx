import type { ReactNode } from "react";
import type { SettingsIntent, SettingsStateSnapshot, SettingsRoute, VsCodeApiLike } from "../../../src/shared/settingsProtocol";
import { useT } from "../i18n/LocaleProvider";

export function SettingsShell({ state, vscodeApi, children }: {
  state: SettingsStateSnapshot;
  vscodeApi: VsCodeApiLike<SettingsIntent>;
  children: ReactNode;
}) {
  const t = useT();
  function navigate(route: SettingsRoute) {
    vscodeApi.postMessage({ type: "settings.ready", messageId: `settings-nav-${Date.now()}`, data: { route } });
  }
  const version = (value?: string | null) => value?.trim() ? `v${value.trim()}` : "vunknown";
  return <div className="tc-settings-shell">
    <aside className="tc-settings-shell__nav">
      <div className="tc-settings-shell__brand">Tomcat {t("settings.title")}</div>
      {(["general", "models", "sessions", "tools", "connectors"] as const).map((route) => {
        const disabled = route === "sessions" || route === "tools";
        return <button key={route} className={`tc-settings-nav__item${state.route === route ? " tc-settings-nav__item--active" : ""}`}
          data-testid={`settings-nav-${route}`} disabled={disabled} aria-current={state.route === route ? "page" : undefined}
          onClick={() => { if (!disabled) navigate(route); }} type="button">{t(`settings.${route}`)}</button>;
      })}
      <div className="tc-settings-shell__version" data-testid="settings-version-footer">
        <div>{t("settings.extensionVersion", { version: version(state.extensionVersion) })}</div>
        <div>{t("settings.serveVersion", { version: version(state.serverVersion) })}</div>
      </div>
    </aside>
    {children}
  </div>;
}
