import { useRef, useState, useEffect } from "react";
import type { SettingsIntent, SettingsStateSnapshot, VsCodeApiLike } from "../../../src/shared/settingsProtocol";
import { isLanguagePreference } from "../../../src/shared/i18n";
import { useT } from "../i18n/LocaleProvider";
import { SettingsShell } from "./SettingsShell";

export function GeneralSettingsView({ state, vscodeApi }: { state: SettingsStateSnapshot; vscodeApi: VsCodeApiLike<SettingsIntent> }) {
  const t = useT();
  const preferences = state.uiPreferences;
  const [pending, setPending] = useState(false);
  const lock = useRef(false);
  useEffect(() => { if (!state.ready || state.languageStatus === "saved" || state.languageStatus === "failed") { lock.current = false; setPending(false); } }, [state.ready, state.languageStatus, preferences]);
  const disabled = !state.ready || !preferences || !state.capabilities.setUiLanguage || pending || state.languageStatus === "saving";
  const languageName = (locale?: string) => locale === "zh-CN" ? "中文" : "English";
  return <SettingsShell state={state} vscodeApi={vscodeApi}>
    <main className="tc-settings-shell__content">
      <header className="tc-settings-shell__header"><h1>{t("settings.general")}</h1></header>
      <section className="tc-settings-group">
        <h2 className="tc-settings-group__title">{t("settings.language")}</h2>
        <div className="tc-connector-form-row">
          <label htmlFor="settings-language">{t("settings.interfaceLanguage")}</label>
          <select className="tc-input" id="settings-language" data-testid="settings-language" disabled={disabled} value={preferences?.language ?? ""}
            onChange={(event) => {
              const language = event.target.value;
              if (lock.current || !isLanguagePreference(language) || language === preferences?.language) return;
              lock.current = true;
              setPending(true);
              vscodeApi.postMessage({ type: "setUiLanguage", messageId: `language-${Date.now()}`, data: { language } });
            }}>
            {!preferences ? <option value="">{t("common.loading")}</option> : null}
            <option value="auto">{t("settings.language.auto", { language: languageName(state.hostLocale) })}</option>
            <option value="zh-CN">中文</option>
            <option value="en">English</option>
          </select>
        </div>
        <p>{t("settings.language.scope")}</p>
        <p className="tc-muted">{t("settings.language.native")}</p>
        {preferences?.envOverride ? <p role="status">{t("settings.language.override", { language: languageName(preferences.effective) })}</p> : null}
        {!preferences || !state.ready ? <div className="tc-banner">
          {t("settings.language.unavailable")}
          <button className="tc-button tc-button--secondary" type="button" onClick={() => vscodeApi.postMessage({ type: "settings.ready", messageId: `language-retry-${Date.now()}`, data: { route: "general" } })}>{t("common.retry")}</button>
        </div> : null}
        {(pending || state.languageStatus) ? <p role="status">{pending || state.languageStatus === "saving" ? t("settings.language.saving") : state.languageStatus === "saved" ? t("settings.language.saved") : t("settings.language.failed")}</p> : null}
        {state.error ? <div role="alert" className="tc-banner tc-banner--warning">{state.error}</div> : null}
      </section>
    </main>
  </SettingsShell>;
}
