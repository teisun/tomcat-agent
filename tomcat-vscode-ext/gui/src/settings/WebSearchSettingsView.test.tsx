import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import type { SettingsIntent, SettingsStateSnapshot } from "../../../src/shared/settingsProtocol";
import { LocaleProvider } from "../i18n/LocaleProvider";
import { WebSearchSettingsView } from "./WebSearchSettingsView";

const state: SettingsStateSnapshot = {
  capabilities: { listModels: false, listProviderKeys: false, removeModel: false, setProviderKey: true, upsertModel: false, webSearchSettings: true },
  route: "tools", ready: true, models: [], providerKeys: [],
  webSearch: { usingDefaults: false, order: ["brave"], backends: [
    { id: "brave", label: "Brave", keyEnv: "BRAVE_API_KEY", canSaveKey: true, keySource: "saved", sharedModelIds: [] },
    { id: "example", label: "New provider", keyEnv: "EXAMPLE_API_KEY", canSaveKey: true, sharedModelIds: ["example/chat"] },
  ] },
};

afterEach(cleanup);

function setup() {
  const postMessage = vi.fn<(intent: SettingsIntent) => void>();
  const view = (snapshot: SettingsStateSnapshot) => <LocaleProvider locale="en"><WebSearchSettingsView state={snapshot} vscodeApi={{ postMessage }} /></LocaleProvider>;
  const component = render(view(state));
  return { postMessage, update: (patch: Partial<SettingsStateSnapshot>) => component.rerender(view({ ...state, ...patch })) };
}

it("renders arbitrary declared providers and saves routing without guessing the result", () => {
  const { postMessage, update } = setup();
  const tab = screen.getByRole("tab", { name: "Web Search", selected: true });
  expect(screen.getByRole("tabpanel", { name: "Web Search" }).id).toBe(tab.getAttribute("aria-controls"));
  const newProvider = screen.getByRole("switch", { name: "Use New provider for web search" });
  fireEvent.click(screen.getByRole("switch", { name: "Use Brave for web search" }));
  expect(postMessage).not.toHaveBeenCalled();
  fireEvent.click(newProvider);
  fireEvent.click(newProvider);
  expect(postMessage).toHaveBeenCalledTimes(1);
  const request = postMessage.mock.calls[0][0];
  expect(request).toMatchObject({ type: "setWebSearchRouting", data: { backends: ["brave", "example"] } });
  expect(newProvider.getAttribute("aria-checked")).toBe("false");
  update({ webSearchReceipt: { requestId: request.messageId, phase: "failed" }, error: "Disk is read-only" });
  expect(screen.getByRole("alert").textContent).toContain("Disk is read-only");
  expect(newProvider.getAttribute("aria-checked")).toBe("false");
  fireEvent.click(newProvider);
  const retry = postMessage.mock.calls[1][0];
  update({ webSearch: { ...state.webSearch!, order: ["brave", "example"] }, webSearchReceipt: { requestId: retry.messageId, phase: "saved" } });
  expect(document.activeElement).toBe(newProvider);
  expect(screen.getByRole("status").textContent).toContain("New provider is now number 2");
  fireEvent.click(screen.getByRole("button", { name: "Move New provider up" }));
  expect(postMessage.mock.lastCall?.[0]).toMatchObject({ data: { backends: ["example", "brave"] } });
});

it("keeps a key private and saves it without enrolling its provider", () => {
  const { postMessage } = setup();
  const row = within(screen.getByTestId("web-search-row-example"));
  fireEvent.click(row.getByRole("button", { name: "Configure · New provider" }));
  const input = row.getByLabelText("API key") as HTMLInputElement;
  expect(input.type).toBe("password");
  expect(input.value).toBe("");
  expect(row.queryByRole("button", { name: "Remove key" })).toBeNull();
  fireEvent.change(input, { target: { value: "synthetic-fixture" } });
  fireEvent.submit(input.form!);
  expect(postMessage.mock.calls.map(([message]) => message)).toEqual([
    expect.objectContaining({ type: "saveSearchKey", data: { backendId: "example", value: "synthetic-fixture" } }),
  ]);
  expect(input.value).toBe("");
  expect(row.getByRole("switch").getAttribute("aria-checked")).toBe("false");
});

it("groups saved key actions and still confirms removal before sending it", () => {
  const { postMessage } = setup();
  const brave = within(screen.getByTestId("web-search-row-brave"));
  const trigger = brave.getByRole("button", { name: "Configured · Brave" });
  const actions = brave.getByTestId("web-search-key-actions-brave");
  expect(trigger.getAttribute("popovertarget")).toBe(actions.id);
  expect(actions.getAttribute("popover")).toBe("auto");
  fireEvent.click(within(actions).getByText("Remove key…"));
  expect(brave.getByText("Remove the saved key for Brave?")).toBeTruthy();
  expect(postMessage).not.toHaveBeenCalled();
  fireEvent.click(brave.getByRole("button", { name: "Cancel" }));
  expect(document.activeElement).toBe(trigger);
  fireEvent.click(within(actions).getByText("Remove key…"));
  fireEvent.click(brave.getByRole("button", { name: "Remove key" }));
  expect(postMessage.mock.lastCall?.[0]).toMatchObject({ type: "clearSearchKey", data: { backendId: "brave" } });
});

it("keeps shared keys in Models and offers direct navigation when a model is missing", () => {
  const { postMessage, update } = setup();
  update({ webSearch: { ...state.webSearch!, backends: [
    ...state.webSearch!.backends.map(row => row.id === "example" ? { ...row, keySource: "saved" as const } : row),
    { id: "openai", label: "OpenAI", host: "api.openai.com", canSaveKey: true, sharedModelIds: [] },
  ] } });
  const shared = within(screen.getByTestId("web-search-row-example"));
  expect(shared.getByRole("button", { name: "Configured · New provider" })).toBeTruthy();
  const actions = within(shared.getByTestId("web-search-key-actions-example"));
  expect(actions.queryByText(/Remove key/)).toBeNull();
  expect(actions.getByText(/Shared with: example\/chat/)).toBeTruthy();
  expect(shared.queryByLabelText("API key")).toBeNull();
  fireEvent.click(actions.getByText("Replace key"));
  expect((shared.getByLabelText("API key") as HTMLInputElement).value).toBe("");
  fireEvent.click(actions.getByText("Open Models"));
  expect(screen.getByText(/Configure a key for api.openai.com in Models/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Open Models · OpenAI" }));
  expect(postMessage.mock.calls.map(([message]) => message)).toEqual([
    expect.objectContaining({ type: "settings.ready", data: { route: "models" } }),
    expect.objectContaining({ type: "settings.ready", data: { route: "models" } }),
  ]);
});

it("shows a missing key as plain status when this plugin cannot use saved keys", () => {
  const { update } = setup();
  update({ webSearch: { ...state.webSearch!, backends: [
    { id: "brave", label: "Brave", host: "api.search.brave.com", keyEnv: "BRAVE_API_KEY", canSaveKey: false, sharedModelIds: [] },
  ] } });
  const row = within(screen.getByTestId("web-search-row-brave"));
  expect(row.queryByRole("button", { name: /Brave$/ })).toBeNull();
  expect(row.getByText("Not configured")).toBeTruthy();
  expect(row.getByText(/Project plugins cannot use saved keys/)).toBeTruthy();
});
