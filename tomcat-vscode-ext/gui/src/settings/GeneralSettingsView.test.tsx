import { act, fireEvent, render, screen, cleanup } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SettingsApp } from "./SettingsApp";
import { LocaleProvider } from "../i18n/LocaleProvider";
import { translate } from "../../../src/shared/i18n";
import type { SettingsStateSnapshot } from "../../../src/shared/settingsProtocol";

const ready = (overrides: Partial<SettingsStateSnapshot> = {}): SettingsStateSnapshot => ({
  capabilities: { listModels: false, listProviderKeys: false, removeModel: false, setProviderKey: false, upsertModel: false, setUiLanguage: true },
  models: [], providerKeys: [], ready: true, route: "general",
  uiPreferences: { language: "auto", effective: "en", envOverride: false }, hostLocale: "en", ...overrides,
});
async function emit(state: SettingsStateSnapshot) {
  await act(async () => { window.dispatchEvent(new MessageEvent("message", { data: { channel: "state", content: state, messageId: "state" } })); });
}
afterEach(cleanup);

describe("General language settings", () => {
  it("disables unavailable preferences and sends one correlated change without optimistic language replacement", async () => {
    const postMessage = vi.fn();
    render(<SettingsApp initialRoute="general" vscodeApi={{ postMessage }} />);
    expect((screen.getByTestId("settings-language") as HTMLSelectElement).disabled).toBe(true);
    await emit(ready());
    const select = screen.getByTestId("settings-language") as HTMLSelectElement;
    expect(select.classList.contains("tc-input")).toBe(true);
    fireEvent.change(select, { target: { value: "zh-CN" } });
    fireEvent.change(select, { target: { value: "en" } });
    expect(postMessage.mock.calls.filter(([m]) => m.type === "setUiLanguage")).toHaveLength(1);
    expect(select.disabled).toBe(true);
    expect(select.value).toBe("auto");
    await emit(ready({ languageStatus: "failed", error: "disk error" }));
    expect(select.disabled).toBe(false);
    expect(select.value).toBe("auto");
    expect(screen.getByRole("alert").textContent).toContain("disk error");
  });
  it("keeps native language choices and environment override across locale changes without removing disabled routes", async () => {
    const api = { postMessage: vi.fn() };
    const view = render(<LocaleProvider locale="en"><SettingsApp initialRoute="general" vscodeApi={api} /></LocaleProvider>);
    await emit(ready({ uiPreferences: { language: "zh-CN", effective: "en", envOverride: true } }));
    const language = screen.getByLabelText(translate("en", "settings.interfaceLanguage"));
    expect(screen.getByText(/TOMCAT__UI__LANGUAGE/)).toBeTruthy();
    expect(screen.getByRole("option", { name: "English" })).toBeTruthy();
    view.rerender(<LocaleProvider locale="zh-CN"><SettingsApp initialRoute="general" vscodeApi={api} /></LocaleProvider>);
    expect(screen.getByTestId("settings-language")).toBe(language);
    expect((screen.getByTestId("settings-nav-sessions") as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByTestId("settings-nav-tools") as HTMLButtonElement).disabled).toBe(true);
  });
  it("keeps model forms mounted when the surrounding locale changes", async () => {
    const api = { postMessage: vi.fn() };
    const { rerender } = render(<LocaleProvider locale="en"><SettingsApp vscodeApi={api} /></LocaleProvider>);
    await emit(ready({ route: "models", capabilities: { listModels: true, listProviderKeys: false, removeModel: false, setProviderKey: false, upsertModel: true } }));
    fireEvent.click(screen.getByTestId("settings-add-model"));
    const dialog = screen.getByRole("dialog");
    rerender(<LocaleProvider locale="zh-CN"><SettingsApp vscodeApi={api} /></LocaleProvider>);
    expect(screen.getByRole("dialog")).toBe(dialog);
    expect(screen.getByTestId("settings-nav-general")).toHaveProperty("disabled", false);
  });
});
