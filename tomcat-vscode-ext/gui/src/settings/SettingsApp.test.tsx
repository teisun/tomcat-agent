import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type {
  SettingsCapabilities,
  SettingsHostFrame,
  SettingsIntent,
  SettingsModelView,
  SettingsStateSnapshot,
  VsCodeApiLike,
} from "../../../src/shared/settingsProtocol";
import { SettingsApp } from "./SettingsApp";
import { LocaleProvider } from "../i18n/LocaleProvider";
import { translate } from "../../../src/shared/i18n";

const en = (key: Parameters<typeof translate>[1], args?: Record<string, string | number>) => translate("en", key, args);

function builtinModel(
  overrides: Partial<SettingsModelView> = {},
): SettingsModelView {
  return {
    api: "openai-responses",
    apiKeyEnv: "OPENAI_API_KEY",
    baseUrl: "https://api.openai.com",
    capabilities: {
      files: true,
      reasoning: true,
      tools: true,
      vision: true,
      webSearch: false,
    },
    contextWindow: 400000,
    id: "gpt-5.4",
    keyPresent: false,
    modelName: "gpt-5.4",
    provider: "openai",
    source: "builtin",
    thinkingFormat: "openai",
    ...overrides,
  };
}

function deepseek(id: string, overrides: Partial<SettingsModelView> = {}): SettingsModelView {
  return builtinModel({
    api: "openai",
    apiKeyEnv: "DEEPSEEK_API_KEY",
    baseUrl: "https://api.deepseek.com",
    id,
    modelName: id,
    provider: "deepseek",
    thinkingFormat: "deepseek",
    ...overrides,
  });
}

function relayModel(name: string, overrides: Partial<SettingsModelView> = {}): SettingsModelView {
  return builtinModel({
    apiKeyEnv: "FCODEX_API_KEY",
    baseUrl: "https://api.fcodex.com",
    id: `fcodex/${name}`,
    keyPresent: true,
    keySource: "saved",
    modelName: name,
    provider: "fcodex",
    source: "user",
    ...overrides,
  });
}

const CONNECT_CAPABILITIES: Partial<SettingsCapabilities> = {
  listRemoteModels: true,
  removeProvider: true,
  setDefaultModel: true,
  setDefaultModelIfUnset: true,
  setUtilityModel: true,
  upsertModels: true,
};

function mount() {
  const postMessage = vi.fn();
  const vscodeApi: VsCodeApiLike<SettingsIntent> = {
    postMessage,
    setState: vi.fn(),
  };
  render(<SettingsApp vscodeApi={vscodeApi} />);
  return { postMessage };
}

async function emitState(content: SettingsStateSnapshot) {
  const frame: SettingsHostFrame = {
    channel: "state",
    content,
    messageId: "settings-state",
  };
  await act(async () => {
    window.dispatchEvent(new MessageEvent("message", { data: frame }));
  });
}

async function emitDomAction(action: {
  kind: "clickTestId" | "setInputValue";
  testId: string;
  value?: string;
}) {
  await act(async () => {
    window.dispatchEvent(new MessageEvent("message", {
      data: {
        channel: "event",
        content: { action, type: "__test.dom_action" },
        messageId: `settings-dom-action-${action.testId}`,
      },
    }));
  });
}

function readyState(
  overrides: Partial<SettingsStateSnapshot> = {},
): SettingsStateSnapshot {
  return {
    capabilities: {
      listModels: true,
      listProviderKeys: true,
      removeModel: true,
      setProviderKey: true,
      upsertModel: true,
    },
    models: [],
    providerKeys: [],
    ready: true,
    route: "models",
    ...overrides,
  };
}

function connectState(overrides: Partial<SettingsStateSnapshot> = {}): SettingsStateSnapshot {
  return readyState({
    ...overrides,
    capabilities: { ...readyState().capabilities, ...CONNECT_CAPABILITIES, ...overrides.capabilities },
  });
}

function sent<T extends SettingsIntent["type"]>(postMessage: ReturnType<typeof vi.fn>, type: T): Array<SettingsIntent & { type: T }> {
  return postMessage.mock.calls
    .map(([message]) => message as SettingsIntent)
    .filter((message): message is SettingsIntent & { type: T } => message.type === type);
}

function input(testId: string): HTMLInputElement {
  return screen.getByTestId(testId) as HTMLInputElement;
}

function protocolOf(modelId: string): string | null | undefined {
  return screen.getByTestId(`settings-model-row-${modelId}`).querySelector(".tc-settings-card__protocol")?.textContent;
}

describe("SettingsApp", () => {
  it("does not diagnose an old CLI before connecting and displays the local extension version", async () => {
    mount();
    await emitState(readyState({ ready: false, extensionVersion: "0.1.89", serverVersion: null, expectedCliVersion: "0.1.73", error: "broken config" }));
    expect(screen.getByTestId("settings-version-footer").textContent).toContain("0.1.89");
    expect(screen.getByTestId("settings-version-footer").textContent).toContain(en("settings.waitingForConnection"));
    expect(screen.queryByText(en("models.versionMissing"))).toBeNull();
    expect(screen.queryByText(en("models.unavailableHint"))).toBeNull();
    expect(screen.getByText("broken config")).toBeTruthy();
  });

  it("posts one ready handshake for the requested route", () => {
    const { postMessage } = mount();
    expect(postMessage).toHaveBeenCalledTimes(1);
    expect(postMessage.mock.calls[0][0]).toMatchObject({ data: { route: "models" }, type: "settings.ready" });
  });

  describe("provider cards", () => {
    it("shows only the connected provider and folds key-less built-ins into the connectable list", async () => {
      mount();
      await emitState(connectState({
        defaultModelId: "deepseek-v4-pro",
        models: [
          deepseek("deepseek-v4-pro", { keyPresent: true, keySource: "saved" }),
          deepseek("deepseek-v4-flash", { keyPresent: true, keySource: "saved" }),
          builtinModel(),
          builtinModel({ api: "anthropic-messages", apiKeyEnv: "ANTHROPIC_API_KEY", baseUrl: "https://api.anthropic.com", id: "claude-opus-5", modelName: "claude-opus-5", provider: "anthropic" }),
        ],
        utilityModelId: "deepseek-v4-flash",
      }));

      expect(screen.queryAllByTestId(/^settings-card-[A-Z0-9_]+$/u).map((card) => card.dataset.testid)).toEqual(["settings-card-DEEPSEEK_API_KEY"]);
      expect(screen.queryByText("claude-opus-5")).toBeNull();
      const card = screen.getByTestId("settings-card-DEEPSEEK_API_KEY");
      expect(within(card).getByText("DeepSeek")).toBeTruthy();
      expect(within(card).getByText("api.deepseek.com")).toBeTruthy();
      expect(screen.getByTestId("settings-card-key-DEEPSEEK_API_KEY").textContent).toBe(en("models.card.keySaved"));
      const pro = screen.getByTestId("settings-model-row-deepseek-v4-pro");
      expect(protocolOf("deepseek-v4-pro")).toBe(en("term.api.openai"));
      expect(within(pro).getByTestId("settings-default-deepseek-v4-pro")).toBeTruthy();
      expect(within(screen.getByTestId("settings-model-row-deepseek-v4-flash")).getByTestId("settings-utility-deepseek-v4-flash")).toBeTruthy();
      expect(within(pro).queryByTestId("settings-delete-deepseek-v4-pro")).toBeNull();

      const connectable = screen.getByTestId("settings-connectable");
      expect(within(connectable).queryAllByRole("button").map((button) => button.dataset.testid)).toEqual([
        "settings-connectable-openai",
        "settings-connectable-anthropic",
        "settings-connectable-kimi",
        "settings-connectable-zhipu",
        "settings-connectable-moonshot",
        "settings-connectable-mimo",
        "settings-connectable-other",
      ]);
    });

    it("keeps one card for a single-key relay and shows each row's own protocol", async () => {
      mount();
      await emitState(connectState({
        models: [
          relayModel("gpt-6.1-sol"),
          relayModel("claude-opus-5-5", { api: "anthropic-messages" }),
        ],
      }));
      const card = screen.getByTestId("settings-card-FCODEX_API_KEY");
      expect(within(card).getByText("Fcodex")).toBeTruthy();
      expect(protocolOf("fcodex/gpt-6.1-sol")).toBe(en("term.api.responses"));
      expect(protocolOf("fcodex/claude-opus-5-5")).toBe(en("term.api.anthropic"));
      expect(screen.getByTestId("settings-delete-fcodex/gpt-6.1-sol")).toBeTruthy();
    });

    it("places default before utility and keeps row details and actions without Set default", async () => {
      mount();
      await emitState(connectState({
        defaultModelId: "gpt-5.4",
        utilityModelId: "gpt-5.2",
        models: [
          builtinModel({ keyPresent: true }),
          builtinModel({ id: "gpt-5.2", keyPresent: true, modelName: "gpt-5.2" }),
          relayModel("gpt-5.5", { keyPresent: false }),
        ],
      }));
      expect(screen.getAllByRole("combobox").map((picker) => picker.getAttribute("data-testid"))).toEqual([
        "settings-default-model-input", "settings-utility-model-input",
      ]);
      expect(screen.queryAllByTestId(/^settings-make-default-/u)).toHaveLength(0);
      const row = screen.getByTestId("settings-model-row-gpt-5.4");
      expect(within(row).getByTestId("settings-default-gpt-5.4")).toBeTruthy();
      expect(within(row).getByRole("button", { name: en("models.details", { name: "gpt-5.4" }) })).toBeTruthy();
      expect(within(row).getByTestId("settings-edit-gpt-5.4")).toBeTruthy();
      expect(screen.getByTestId("settings-delete-fcodex/gpt-5.5")).toBeTruthy();
      expect(screen.getByTestId("settings-utility-gpt-5.2")).toBeTruthy();

      await emitState(readyState({ defaultModelId: "gpt-5.4", models: [builtinModel({ keyPresent: true })] }));
      expect(screen.queryByTestId("settings-default-gpt-5.4")).toBeNull();
      expect(input("settings-default-model-input").disabled).toBe(true);
    });

    it("fills a missing key inline and asks for the card's first model as the first default", async () => {
      const { postMessage } = mount();
      await emitState(connectState({
        defaultModelId: "",
        models: [relayModel("gpt-5.5", { keyPresent: false, keySource: null }), relayModel("gpt-5.4", { keyPresent: false, keySource: null })],
      }));
      expect(screen.getByTestId("settings-card-key-FCODEX_API_KEY").textContent).toBe(en("models.card.keyMissing"));

      fireEvent.click(screen.getByTestId("settings-card-fill-key-FCODEX_API_KEY"));
      fireEvent.click(screen.getByRole("button", { name: en("common.cancel") }));
      expect(sent(postMessage, "setProviderKey")).toHaveLength(0);

      fireEvent.click(screen.getByTestId("settings-card-fill-key-FCODEX_API_KEY"));
      const keyInput = input("settings-card-key-input-FCODEX_API_KEY");
      expect(keyInput.type).toBe("password");
      fireEvent.change(keyInput, { target: { value: "relay-secret" } });
      fireEvent.click(screen.getByTestId("settings-card-key-save-FCODEX_API_KEY"));
      expect(sent(postMessage, "setProviderKey").map((message) => message.data)).toEqual([
        { defaultModelId: "fcodex/gpt-5.5", envName: "FCODEX_API_KEY", value: "relay-secret" },
      ]);
      expect(screen.queryByTestId("settings-card-key-input-FCODEX_API_KEY")).toBeNull();
    });

    it("replaces a card's key and removes a card only after confirming the listed models", async () => {
      const { postMessage } = mount();
      await emitState(connectState({
        models: [
          relayModel("gpt-6.1-sol"),
          relayModel("claude-opus-5-5", { api: "anthropic-messages" }),
          builtinModel({ api: "anthropic-messages", apiKeyEnv: "ANTHROPIC_API_KEY", baseUrl: "https://api.anthropic.com", id: "claude-opus-5", keyPresent: true, keySource: "environment", modelName: "claude-opus-5", provider: "anthropic" }),
        ],
      }));

      fireEvent.click(screen.getByTestId("settings-card-menu-FCODEX_API_KEY"));
      fireEvent.click(screen.getByTestId("settings-card-replace-key-FCODEX_API_KEY"));
      const replace = screen.getByTestId("settings-replace-key-dialog");
      expect(within(replace).getByText("fcodex/gpt-6.1-sol")).toBeTruthy();
      expect(within(replace).getByText("fcodex/claude-opus-5-5")).toBeTruthy();
      fireEvent.change(input("settings-replace-key-input"), { target: { value: "new-relay-secret" } });
      fireEvent.click(screen.getByTestId("settings-replace-key-save"));
      expect(sent(postMessage, "setProviderKey").map((message) => message.data)).toEqual([
        { envName: "FCODEX_API_KEY", value: "new-relay-secret" },
      ]);

      fireEvent.click(screen.getByTestId("settings-card-menu-FCODEX_API_KEY"));
      fireEvent.click(screen.getByTestId("settings-card-remove-FCODEX_API_KEY"));
      let confirmation = screen.getByTestId("settings-remove-provider-confirmation");
      expect(within(confirmation).getByText(en("models.removeProvider.title", { name: "Fcodex" }))).toBeTruthy();
      expect(within(confirmation).getByText("fcodex/gpt-6.1-sol")).toBeTruthy();
      expect(within(confirmation).getByText("fcodex/claude-opus-5-5")).toBeTruthy();
      fireEvent.click(within(confirmation).getByRole("button", { name: en("common.cancel") }));
      expect(sent(postMessage, "removeProvider")).toHaveLength(0);

      fireEvent.click(screen.getByTestId("settings-card-menu-FCODEX_API_KEY"));
      fireEvent.click(screen.getByTestId("settings-card-remove-FCODEX_API_KEY"));
      confirmation = screen.getByTestId("settings-remove-provider-confirmation");
      fireEvent.click(screen.getByTestId("settings-remove-provider-confirm"));
      expect(sent(postMessage, "removeProvider").map((message) => message.data)).toEqual([{ keyEnv: "FCODEX_API_KEY" }]);
      expect(screen.queryByTestId("settings-remove-provider-confirmation")).toBeNull();

      expect(screen.getByTestId("settings-card-key-ANTHROPIC_API_KEY").textContent)
        .toBe(en("models.card.keyEnv", { name: "ANTHROPIC_API_KEY" }));
      fireEvent.click(screen.getByTestId("settings-card-menu-ANTHROPIC_API_KEY"));
      expect(input("settings-card-remove-ANTHROPIC_API_KEY").disabled).toBe(true);
      expect(screen.getByText(en("models.removeProvider.envKey", { name: "ANTHROPIC_API_KEY" }))).toBeTruthy();
    });

    it("disables provider removal and connection on a CLI without the new commands", async () => {
      mount();
      await emitState(readyState({ models: [relayModel("gpt-6.1-sol")] }));
      expect((screen.getByTestId("settings-connect-provider") as HTMLButtonElement).disabled).toBe(true);
      expect(screen.getByTestId("settings-connect-unsupported").textContent).toBe(en("models.connect.unsupported"));
      expect(screen.queryByTestId("settings-card-add-model-FCODEX_API_KEY")).toBeNull();
      fireEvent.click(screen.getByTestId("settings-card-menu-FCODEX_API_KEY"));
      expect(input("settings-card-remove-FCODEX_API_KEY").disabled).toBe(true);
      expect(screen.getByText(en("models.removeProvider.unsupported"))).toBeTruthy();
    });

    it("confirms a single model deletion, ignores a delayed receipt, and shows the real outcome", async () => {
      const { postMessage } = mount();
      const userModel = relayModel("remove-me");
      const state = connectState({ models: [relayModel("keep-me"), userModel] });
      await emitState(state);
      postMessage.mockClear();

      fireEvent.click(screen.getByTestId("settings-delete-fcodex/remove-me"));
      const confirmation = screen.getByRole("alertdialog");
      expect(within(confirmation).getByText(en("models.deleteQuestion", { name: "fcodex/remove-me" }))).toBeTruthy();
      expect(postMessage).not.toHaveBeenCalled();
      fireEvent.click(screen.getByTestId("settings-delete-model-confirm"));
      expect(sent(postMessage, "removeModel").map((message) => message.data)).toEqual([{ modelId: "fcodex/remove-me" }]);
      expect(screen.getByTestId("settings-delete-fcodex/remove-me").textContent).toContain(en("models.deleting", { name: "fcodex/remove-me" }));

      await emitState({ ...state, modelRemovalReceipt: { modelId: "fcodex/other", success: true, warnings: [] } });
      expect(screen.getByTestId("settings-delete-fcodex/remove-me").textContent).toContain(en("models.deleting", { name: "fcodex/remove-me" }));

      await emitState({
        ...state,
        modelRemovalReceipt: { modelId: "fcodex/remove-me", success: true, warnings: ["A later catalog refresh is still required."] },
        models: [relayModel("keep-me")],
        status: "Model removed with warnings.",
        warnings: ["A later catalog refresh is still required."],
      });
      expect(screen.queryByTestId("settings-model-row-fcodex/remove-me")).toBeNull();
      expect(screen.getByText("Model removed with warnings.")).toBeTruthy();
      expect(screen.getByText("A later catalog refresh is still required.")).toBeTruthy();
    });
  });

  describe("connect provider dialog", () => {
    it("connects a preset with its bundled models locked in and names the first default", async () => {
      const { postMessage } = mount();
      const state = connectState({
        defaultModelId: "",
        models: [deepseek("deepseek-v4-pro"), deepseek("deepseek-v4-flash")],
      });
      await emitState(state);
      fireEvent.click(screen.getByTestId("settings-connectable-deepseek"));
      const dialog = screen.getByTestId("settings-connect-dialog");
      expect((within(dialog).getByTestId("settings-connect-preset") as HTMLSelectElement).value).toBe("deepseek");
      expect(input("settings-connect-address").readOnly).toBe(true);
      expect(input("settings-connect-address").value).toBe("https://api.deepseek.com");
      const bundled = input("settings-connect-model-deepseek-v4-pro");
      expect(bundled.checked).toBe(true);
      expect(bundled.disabled).toBe(true);
      expect(bundled.closest("label")?.title).toBe(en("models.connect.included"));
      expect(within(dialog).queryAllByTestId(/^settings-connect-protocol-/u)).toHaveLength(0);
      expect(within(dialog).queryByTestId("settings-connect-default-hint")).toBeNull();

      fireEvent.click(screen.getByTestId("settings-connect-save"));
      expect(within(dialog).getByText(en("models.connect.validate.keyRequired"))).toBeTruthy();
      expect(sent(postMessage, "upsertModels")).toHaveLength(0);

      fireEvent.change(input("settings-connect-api-key"), { target: { value: "deepseek-secret" } });
      expect(screen.getByTestId("settings-connect-default-hint").textContent)
        .toBe(en("models.connect.defaultAfterSave", { name: "deepseek-v4-pro" }));
      fireEvent.change(input("settings-connect-manual-input"), { target: { value: "claude-x" } });
      fireEvent.click(screen.getByTestId("settings-connect-manual-add"));
      expect(input("settings-connect-model-claude-x").checked).toBe(true);
      expect(within(dialog).queryByTestId("settings-connect-protocol-claude-x")).toBeNull();
      fireEvent.click(screen.getByTestId("settings-connect-save"));

      const [request] = sent(postMessage, "upsertModels");
      expect(request.data.providerKey).toEqual({ envName: "DEEPSEEK_API_KEY", value: "deepseek-secret" });
      expect(request.data.defaultModelId).toBe("deepseek-v4-pro");
      expect(request.data.setAsDefault).toBeUndefined();
      expect(request.data.models).toEqual([expect.objectContaining({
        api: "openai",
        apiKeyEnv: "DEEPSEEK_API_KEY",
        baseUrl: "https://api.deepseek.com",
        contextWindow: null,
        id: "deepseek/claude-x",
        maxOutputTokens: null,
        modelName: "claude-x",
        provider: "deepseek",
        supportedSpeeds: ["fast"],
        thinkingFormat: "deepseek",
      })]);
      expect(sent(postMessage, "setProviderKey")).toHaveLength(0);

      await emitState({ ...state, error: "key denied", modelSaveReceipt: { requestId: request.messageId, success: false } });
      expect(screen.getByTestId("settings-connect-dialog")).toBe(dialog);
      expect(input("settings-connect-api-key").value).toBe("deepseek-secret");
      expect(within(dialog).getByRole("status").textContent).toContain("key denied");
      await emitState({ ...state, modelSaveReceipt: { requestId: request.messageId, success: true } });
      expect(screen.queryByTestId("settings-connect-dialog")).toBeNull();
    });

    it("lets a single-key relay mix protocols per model in one connection", async () => {
      const { postMessage } = mount();
      const state = connectState({
        defaultModelId: "",
        models: [
          builtinModel({ id: "openai/gpt-6.1-sol", modelName: "gpt-6.1-sol", supportedSpeeds: ["fast"] }),
          builtinModel({ api: "anthropic-messages", apiKeyEnv: "ANTHROPIC_API_KEY", baseUrl: "https://api.anthropic.com", id: "anthropic/claude-opus-5-5", modelName: "claude-opus-5-5", provider: "anthropic", thinkingFormat: "anthropic-adaptive" }),
        ],
      });
      await emitState(state);
      fireEvent.click(screen.getByTestId("settings-connectable-other"));
      fireEvent.change(input("settings-connect-address"), { target: { value: "api.fcodex.com" } });
      fireEvent.change(input("settings-connect-api-key"), { target: { value: "relay-secret" } });
      fireEvent.click(screen.getByTestId("settings-connect-fetch"));

      const [fetch] = sent(postMessage, "listRemoteModels");
      expect(fetch.data).toEqual({ api: "openai", apiKey: "relay-secret", baseUrl: "https://api.fcodex.com" });
      expect(screen.getByTestId("settings-connect-fetch").textContent).toBe(en("models.connect.fetching"));
      await emitState({ ...state, remoteModelsReceipt: { models: ["gpt-6.1-sol", "claude-opus-5-5", "my-relay-model", "deepseek-chat"], requestId: fetch.messageId } });

      for (const name of ["gpt-6.1-sol", "claude-opus-5-5", "my-relay-model", "deepseek-chat"]) {
        expect(input(`settings-connect-model-${name}`).checked).toBe(false);
      }
      expect(screen.queryAllByTestId(/^settings-connect-protocol-/u)).toHaveLength(0);
      for (const name of ["gpt-6.1-sol", "claude-opus-5-5", "my-relay-model"]) {
        fireEvent.click(input(`settings-connect-model-${name}`));
      }
      expect((screen.getByTestId("settings-connect-protocol-gpt-6.1-sol") as HTMLSelectElement).value).toBe("openai-responses");
      expect((screen.getByTestId("settings-connect-protocol-claude-opus-5-5") as HTMLSelectElement).value).toBe("anthropic-messages");
      expect((screen.getByTestId("settings-connect-protocol-my-relay-model") as HTMLSelectElement).value).toBe("openai");
      expect(screen.getByTestId("settings-connect-default-hint").textContent)
        .toBe(en("models.connect.defaultAfterSave", { name: "fcodex/gpt-6.1-sol" }));

      fireEvent.change(screen.getByTestId("settings-connect-search"), { target: { value: "claude" } });
      expect(screen.queryByTestId("settings-connect-model-gpt-6.1-sol")).toBeNull();
      fireEvent.change(screen.getByTestId("settings-connect-search"), { target: { value: "" } });

      fireEvent.click(screen.getByTestId("settings-connect-save"));
      const [request] = sent(postMessage, "upsertModels");
      expect(request.data.providerKey).toEqual({ envName: "FCODEX_API_KEY", value: "relay-secret" });
      expect(request.data.models.map(({ api, apiKeyEnv, id, supportedSpeeds }) => ({ api, apiKeyEnv, id, supportedSpeeds }))).toEqual([
        { api: "openai-responses", apiKeyEnv: "FCODEX_API_KEY", id: "fcodex/gpt-6.1-sol", supportedSpeeds: ["fast"] },
        { api: "anthropic-messages", apiKeyEnv: "FCODEX_API_KEY", id: "fcodex/claude-opus-5-5", supportedSpeeds: ["fast"] },
        { api: "openai", apiKeyEnv: "FCODEX_API_KEY", id: "fcodex/my-relay-model", supportedSpeeds: null },
      ]);
      expect(request.data.models[1]).toMatchObject({ capabilities: { files: true }, thinkingFormat: "anthropic-adaptive" });
      expect(request.data.models[2]).toMatchObject({
        capabilities: { files: true, reasoning: true, tools: true, vision: true, webSearch: false },
        contextWindow: null,
        maxOutputTokens: null,
        thinkingFormat: "openai",
      });
    });

    it("asks whether a known address reuses the saved key or creates a second card", async () => {
      const { postMessage } = mount();
      const state = connectState({ defaultModelId: "fcodex/gpt-6.1-sol", models: [relayModel("gpt-6.1-sol")] });
      await emitState(state);
      fireEvent.click(screen.getByTestId("settings-connectable-other"));
      fireEvent.change(input("settings-connect-address"), { target: { value: "https://api.fcodex.com/" } });
      expect(input("settings-connect-same-FCODEX_API_KEY").checked).toBe(true);
      expect(screen.queryByTestId("settings-connect-api-key")).toBeNull();
      expect(input("settings-connect-model-gpt-6.1-sol").disabled).toBe(true);
      fireEvent.change(input("settings-connect-manual-input"), { target: { value: "claude-opus-5-5" } });
      fireEvent.click(screen.getByTestId("settings-connect-manual-add"));
      expect((screen.getByTestId("settings-connect-protocol-claude-opus-5-5") as HTMLSelectElement).value).toBe("anthropic-messages");
      fireEvent.click(screen.getByTestId("settings-connect-save"));

      const [reuse] = sent(postMessage, "upsertModels");
      expect(reuse.data.providerKey).toBeUndefined();
      expect(reuse.data.defaultModelId).toBeUndefined();
      expect(reuse.data.models.map((model) => [model.id, model.apiKeyEnv])).toEqual([["fcodex/claude-opus-5-5", "FCODEX_API_KEY"]]);
      await emitState({ ...state, error: "write failed", modelSaveReceipt: { requestId: reuse.messageId, success: false } });

      fireEvent.click(screen.getByTestId("settings-connect-same-new"));
      expect(screen.getByTestId("settings-connect-same-new").closest("label")?.textContent)
        .toBe(en("models.connect.useNewKey", { name: "Fcodex 2" }));
      fireEvent.change(input("settings-connect-api-key"), { target: { value: "second-secret" } });
      fireEvent.click(screen.getByTestId("settings-connect-save"));
      const [, second] = sent(postMessage, "upsertModels");
      expect(second.data.providerKey).toEqual({ envName: "FCODEX_2_API_KEY", value: "second-secret" });
      expect(second.data.models.map((model) => model.apiKeyEnv)).toEqual(["FCODEX_2_API_KEY"]);
    });

    it("keeps manual entry usable after a failed fetch and rejects duplicate names", async () => {
      const { postMessage } = mount();
      const state = connectState({ models: [] });
      await emitState(state);
      fireEvent.click(screen.getByTestId("settings-connectable-other"));
      fireEvent.click(screen.getByTestId("settings-connect-fetch"));
      expect(screen.getByText(en("models.validate.baseRequired"))).toBeTruthy();
      expect(sent(postMessage, "listRemoteModels")).toHaveLength(0);

      fireEvent.change(input("settings-connect-address"), { target: { value: "https://user:pw@relay.example.com" } });
      fireEvent.click(screen.getByTestId("settings-connect-fetch"));
      expect(screen.getByText(en("models.validate.baseCredentials"))).toBeTruthy();

      fireEvent.change(input("settings-connect-address"), { target: { value: "http://localhost:11434/v1" } });
      fireEvent.click(screen.getByTestId("settings-connect-fetch"));
      const [fetch] = sent(postMessage, "listRemoteModels");
      expect(fetch.data).toEqual({ api: "openai", baseUrl: "http://localhost:11434/v1" });
      await emitState({ ...state, remoteModelsReceipt: { failure: "unsupported", requestId: fetch.messageId } });
      expect(screen.getByTestId("settings-connect-fetch-failed").textContent).toBe(en("models.connect.failed.unsupported"));

      fireEvent.change(input("settings-connect-manual-input"), { target: { value: "qwen3-coder" } });
      fireEvent.keyDown(input("settings-connect-manual-input"), { key: "Enter" });
      expect(input("settings-connect-model-qwen3-coder").checked).toBe(true);
      fireEvent.change(input("settings-connect-manual-input"), { target: { value: "qwen3-coder" } });
      fireEvent.click(screen.getByTestId("settings-connect-manual-add"));
      expect(screen.getByText(en("models.connect.validate.duplicate", { name: "qwen3-coder" }))).toBeTruthy();
      expect(screen.getAllByTestId("settings-connect-model-qwen3-coder")).toHaveLength(1);
    });

    it("pre-checks an explicit default when the existing default is unusable", async () => {
      const { postMessage } = mount();
      await emitState(connectState({ defaultModelId: "gpt-5.4", models: [builtinModel()] }));
      fireEvent.click(screen.getByTestId("settings-connectable-other"));
      fireEvent.change(input("settings-connect-address"), { target: { value: "https://relay.example.com" } });
      fireEvent.change(input("settings-connect-api-key"), { target: { value: "relay-secret" } });
      fireEvent.change(input("settings-connect-manual-input"), { target: { value: "relay-model" } });
      fireEvent.click(screen.getByTestId("settings-connect-manual-add"));
      expect(screen.queryByTestId("settings-connect-default-hint")).toBeNull();
      expect(input("settings-connect-set-default").checked).toBe(true);
      fireEvent.click(screen.getByTestId("settings-connect-save"));
      const [request] = sent(postMessage, "upsertModels");
      expect(request.data).toMatchObject({ defaultModelId: "example/relay-model", setAsDefault: true });
    });

    it("adds models to an existing card without asking for its key, and cancel writes nothing", async () => {
      const { postMessage } = mount();
      await emitState(connectState({ models: [relayModel("gpt-6.1-sol")] }));
      fireEvent.click(screen.getByTestId("settings-card-add-model-FCODEX_API_KEY"));
      const dialog = screen.getByTestId("settings-connect-dialog");
      expect(within(dialog).getByRole("heading").textContent).toBe(en("models.connect.addTitle", { name: "Fcodex" }));
      expect(within(dialog).getByText(`api.fcodex.com · ${en("models.card.keySaved")}`)).toBeTruthy();
      expect(within(dialog).queryByTestId("settings-connect-api-key")).toBeNull();
      expect(within(dialog).queryByTestId("settings-connect-preset")).toBeNull();
      expect(input("settings-connect-model-gpt-6.1-sol").closest("label")?.title).toBe(en("models.connect.existing"));

      fireEvent.click(screen.getByTestId("settings-connect-fetch"));
      expect(sent(postMessage, "listRemoteModels")[0].data).toEqual({ api: "openai", apiKeyEnv: "FCODEX_API_KEY", baseUrl: "https://api.fcodex.com" });
      fireEvent.change(input("settings-connect-manual-input"), { target: { value: "draft" } });
      fireEvent.click(screen.getByTestId("settings-connect-manual-add"));
      fireEvent.click(screen.getByTestId("settings-connect-cancel"));
      expect(screen.queryByTestId("settings-connect-dialog")).toBeNull();
      expect(sent(postMessage, "upsertModels")).toHaveLength(0);
      expect(sent(postMessage, "setProviderKey")).toHaveLength(0);
    });

    it("adds models to a relay card at the relay address even when a moved built-in is listed first", async () => {
      const { postMessage } = mount();
      const moved = builtinModel({ apiKeyEnv: "FCODEX_API_KEY", id: "gpt-5.6", keyPresent: true, keySource: "saved", modelName: "gpt-5.6" });
      await emitState(connectState({ models: [moved, relayModel("gpt-6.1-sol")] }));
      fireEvent.click(screen.getByTestId("settings-card-add-model-FCODEX_API_KEY"));
      const dialog = screen.getByTestId("settings-connect-dialog");
      expect(within(dialog).getByRole("heading").textContent).toBe(en("models.connect.addTitle", { name: "Fcodex" }));
      fireEvent.change(input("settings-connect-manual-input"), { target: { value: "relay-model" } });
      fireEvent.click(screen.getByTestId("settings-connect-manual-add"));
      fireEvent.click(screen.getByTestId("settings-connect-save"));
      const [request] = sent(postMessage, "upsertModels");
      expect(request.data.models.map(({ apiKeyEnv, baseUrl, id, provider }) => ({ apiKeyEnv, baseUrl, id, provider }))).toEqual([
        { apiKeyEnv: "FCODEX_API_KEY", baseUrl: "https://api.fcodex.com", id: "fcodex/relay-model", provider: "fcodex" },
      ]);
    });

    it("keeps the dialog draft and hidden key across a locale switch", async () => {
      const api = { postMessage: vi.fn() };
      const view = render(<LocaleProvider locale="en"><SettingsApp vscodeApi={api} /></LocaleProvider>);
      await emitState(connectState({ models: [deepseek("deepseek-v4-pro")] }));
      fireEvent.click(screen.getByTestId("settings-connect-provider"));
      const dialog = screen.getByTestId("settings-connect-dialog");
      const keyInput = input("settings-connect-api-key");
      fireEvent.change(keyInput, { target: { value: "private-secret" } });
      view.rerender(<LocaleProvider locale="zh-CN"><SettingsApp vscodeApi={api} /></LocaleProvider>);
      expect(screen.getByTestId("settings-connect-dialog")).toBe(dialog);
      expect(input("settings-connect-api-key")).toBe(keyInput);
      expect(keyInput.value).toBe("private-secret");
      expect(keyInput.type).toBe("password");
      expect(screen.getByText(translate("zh-CN", "models.connect.title"))).toBeTruthy();
      expect(api.postMessage.mock.calls.filter(([message]) => message.type === "settings.ready")).toHaveLength(1);
    });
  });

  describe("model edit form", () => {
    it("edits a built-in at its real address with no key field and saves the full entry", async () => {
      const { postMessage } = mount();
      await emitState(connectState({ models: [builtinModel({ id: "gpt-5.6", keyPresent: true, maxOutputTokens: 128000, modelName: "gpt-5.6" })] }));
      fireEvent.click(screen.getByTestId("settings-edit-gpt-5.6"));
      const form = screen.getByTestId("settings-model-form");
      expect(input("settings-model-base-url").value).toBe("https://api.openai.com");
      expect(screen.getByTestId("settings-model-address-preset").textContent).toBe(en("models.addressPreset", { name: "OpenAI" }));
      expect(form.querySelector('input[type="password"]')).toBeNull();
      expect(within(form).queryByRole("button", { name: en("models.delete") })).toBeNull();
      expect(within(form).queryByRole("tab")).toBeNull();

      fireEvent.click(screen.getByTestId("settings-model-advanced"));
      expect(input("settings-model-alias").readOnly).toBe(true);
      expect(input("settings-context-window").value).toBe("400000");
      expect(screen.queryByTestId("settings-capability-unprobed")).toBeNull();
      fireEvent.click(screen.getByTestId("settings-save-model"));
      expect(sent(postMessage, "setProviderKey")).toHaveLength(0);
      expect(sent(postMessage, "upsertModel").map((message) => message.data)).toEqual([{
        model: expect.objectContaining({
          api: "openai-responses",
          apiKeyEnv: "OPENAI_API_KEY",
          baseUrl: "https://api.openai.com",
          contextWindow: 400000,
          id: "gpt-5.6",
          maxOutputTokens: 128000,
          modelName: "gpt-5.6",
          provider: "openai",
          supportedSpeeds: null,
        }),
      }]);
    });

    it("labels unknown models honestly, keeps runtime-default limits empty, and saves Fast only when ticked", async () => {
      const { postMessage } = mount();
      const state = connectState({
        models: [
          relayModel("my-relay-model", { api: "anthropic-messages", contextWindow: null, maxOutputTokens: null }),
          relayModel("gpt-5.5", { contextWindow: null }),
          builtinModel({ id: "gpt-5.5", modelName: "gpt-5.5" }),
        ],
      });
      await emitState(state);
      fireEvent.click(screen.getByTestId("settings-edit-fcodex/my-relay-model"));
      fireEvent.click(screen.getByTestId("settings-model-advanced"));
      expect(screen.getByTestId("settings-capability-unprobed").textContent).toBe(en("models.capabilityUnprobed"));
      expect(input("settings-context-window").placeholder).toBe(en("models.contextAuto"));
      expect(input("settings-max-output-tokens").value).toBe("");
      expect(input("settings-supported-speeds").checked).toBe(false);

      fireEvent.change(input("settings-max-output-tokens"), { target: { value: "lots" } });
      fireEvent.click(screen.getByTestId("settings-save-model"));
      expect(screen.getByText(en("models.validate.tokens"))).toBeTruthy();
      expect(sent(postMessage, "upsertModel")).toHaveLength(0);

      fireEvent.change(input("settings-max-output-tokens"), { target: { value: "" } });
      fireEvent.click(input("settings-supported-speeds"));
      fireEvent.click(screen.getByTestId("settings-save-model"));
      const [saved] = sent(postMessage, "upsertModel");
      expect(saved.data.model).toMatchObject({
        api: "anthropic-messages",
        capabilities: { files: true },
        contextWindow: null,
        maxOutputTokens: null,
        supportedSpeeds: ["fast"],
      });
      await emitState({ ...state, modelSaveReceipt: { requestId: saved.messageId, success: true } });
      expect(screen.queryByTestId("settings-model-form")).toBeNull();

      fireEvent.click(screen.getByTestId("settings-edit-fcodex/gpt-5.5"));
      fireEvent.click(screen.getByTestId("settings-model-advanced"));
      expect(screen.queryByTestId("settings-capability-unprobed")).toBeNull();
      fireEvent.click(screen.getByTestId("settings-save-model"));
      expect(sent(postMessage, "upsertModel")[1].data.model.supportedSpeeds).toBeNull();
    });

    it("deletes a user model from its edit form after confirmation", async () => {
      const { postMessage } = mount();
      const state = connectState({ models: [relayModel("remove-me")] });
      await emitState(state);
      fireEvent.click(screen.getByTestId("settings-edit-fcodex/remove-me"));
      fireEvent.click(within(screen.getByTestId("settings-model-form")).getByRole("button", { name: en("models.delete") }));
      fireEvent.click(screen.getByTestId("settings-delete-model-confirm"));
      expect(sent(postMessage, "removeModel").map((message) => message.data)).toEqual([{ modelId: "fcodex/remove-me" }]);
      await emitState({ ...state, modelRemovalReceipt: { modelId: "fcodex/remove-me", success: true, warnings: [] }, models: [] });
      expect(screen.queryByTestId("settings-model-form")).toBeNull();
    });
  });

  describe("model role picker", () => {
    const models = [
      deepseek("deepseek-v4-pro", { keyPresent: true, keySource: "saved" }),
      deepseek("deepseek-v4-flash", { keyPresent: true, keySource: "saved" }),
      builtinModel(),
    ];

    it("shows the current role and switches it to another connected model", async () => {
      const { postMessage } = mount();
      await emitState(connectState({ models, utilityModelId: "deepseek-v4-flash" }));
      const picker = input("settings-utility-model-input");
      expect(picker.value).toBe("deepseek-v4-flash · DeepSeek");
      fireEvent.focus(picker);
      expect(screen.queryByTestId("settings-utility-option-gpt-5.4")).toBeNull();
      fireEvent.click(screen.getByTestId("settings-utility-option-deepseek-v4-pro"));
      expect(sent(postMessage, "setUtilityModel").map((message) => message.data)).toEqual([{ modelId: "deepseek-v4-pro" }]);
    });

    it("searches across cards, commits with the keyboard, and marks only the pointed row", async () => {
      const { postMessage } = mount();
      const withRelay = [...models, relayModel("gpt-6.1-sol"), relayModel("claude-opus-5-5", { api: "anthropic-messages" })];
      await emitState(connectState({ models: withRelay, utilityModelId: "deepseek-v4-flash" }));
      expect(screen.getByTestId("settings-utility-deepseek-v4-flash")).toBeTruthy();
      expect(screen.queryByTestId("settings-utility-deepseek-v4-pro")).toBeNull();
      expect(screen.queryByTestId("settings-utility-fcodex/claude-opus-5-5")).toBeNull();

      const picker = input("settings-utility-model-input");
      fireEvent.focus(picker);
      fireEvent.change(picker, { target: { value: "opus" } });
      expect(screen.getAllByRole("option").map((option) => option.getAttribute("data-testid"))).toEqual(["settings-utility-option-fcodex/claude-opus-5-5"]);
      fireEvent.keyDown(picker, { key: "Escape" });
      expect(screen.queryByRole("listbox")).toBeNull();
      expect(sent(postMessage, "setUtilityModel")).toEqual([]);

      fireEvent.change(picker, { target: { value: "fcodex" } });
      fireEvent.keyDown(picker, { key: "ArrowDown" });
      fireEvent.keyDown(picker, { key: "Enter" });
      expect(sent(postMessage, "setUtilityModel").map((message) => message.data)).toEqual([{ modelId: "fcodex/claude-opus-5-5" }]);
      expect(picker.value).toBe("deepseek-v4-flash · DeepSeek");

      await emitState(connectState({ models: withRelay, utilityModelId: "fcodex/claude-opus-5-5" }));
      expect(screen.getByTestId("settings-utility-fcodex/claude-opus-5-5")).toBeTruthy();
      expect(screen.queryByTestId("settings-utility-deepseek-v4-flash")).toBeNull();
      expect(input("settings-utility-model-input").value).toBe("fcodex/claude-opus-5-5 · Fcodex");
    });

    it.each(["default", "utility"] as const)("preserves %s role values, messages and availability", async (role) => {
      const { postMessage } = mount();
      const field = role === "default" ? "defaultModelId" : "utilityModelId";
      const intent = role === "default" ? "setDefaultModel" : "setUtilityModel";
      const otherIntent = role === "default" ? "setUtilityModel" : "setDefaultModel";
      const state = connectState({ models, defaultModelId: "deepseek-v4-pro", utilityModelId: "deepseek-v4-pro" });
      const pickerId = `settings-${role}-model-input`;
      await emitState(state);
      const picker = input(pickerId);
      expect(picker.value).toBe("deepseek-v4-pro · DeepSeek");
      fireEvent.focus(picker);
      fireEvent.click(screen.getByTestId(`settings-${role}-option-deepseek-v4-flash`));
      expect(sent(postMessage, intent).map((message) => message.data)).toEqual([{ modelId: "deepseek-v4-flash" }]);
      expect(sent(postMessage, otherIntent)).toHaveLength(0);
      expect(picker.value).toBe("deepseek-v4-pro · DeepSeek");
      await emitState({ ...state, [field]: "deepseek-v4-flash" });
      expect(picker.value).toBe("deepseek-v4-flash · DeepSeek");
      const otherRole = role === "default" ? "utility" : "default";
      expect(input(`settings-${otherRole}-model-input`).value).toBe("deepseek-v4-pro · DeepSeek");

      await emitState({ ...state, [field]: "" });
      expect(picker.value).toBe(en(`models.${role}.unset`));
      expect(screen.queryByTestId(`settings-${role}-model-problem`)).toBeNull();
      await emitState({ ...state, [field]: "missing-model" });
      expect(picker.value).toBe("missing-model");
      expect(screen.getByTestId(`settings-${role}-model-problem`).textContent).toBe(en(`models.${role}.missing`, { name: "missing-model" }));
      await emitState({ ...state, [field]: "gpt-5.4" });
      expect(picker.value).toBe("gpt-5.4");
      expect(screen.getByTestId(`settings-${role}-model-problem`).textContent).toBe(en(`models.${role}.notConnected`, { name: "OpenAI" }));
      await emitState({ ...state, capabilities: { ...state.capabilities, [intent]: false } });
      expect(picker.disabled).toBe(true);
      expect(picker.title).toBe(en(`models.${role}.readOnly`));
      expect(screen.getByTestId(`settings-${role}-model-toggle`)).toHaveProperty("disabled", true);
      await emitState({ ...state, [field]: undefined });
      expect(screen.queryByTestId(`settings-${role}-model`)).toBeNull();
    });

    it("allows both roles to share a model without sharing ARIA listboxes", async () => {
      mount();
      await emitState(connectState({ models, defaultModelId: "deepseek-v4-pro", utilityModelId: "deepseek-v4-pro" }));
      const defaultPicker = input("settings-default-model-input");
      const utilityPicker = input("settings-utility-model-input");
      expect(defaultPicker.value).toBe(utilityPicker.value);
      fireEvent.focus(defaultPicker);
      const defaultListId = defaultPicker.getAttribute("aria-controls");
      expect(document.getElementById(defaultListId!)).toHaveProperty("role", "listbox");
      fireEvent.keyDown(defaultPicker, { key: "Tab" });
      fireEvent.focus(utilityPicker);
      const utilityListId = utilityPicker.getAttribute("aria-controls");
      expect(utilityListId).not.toBe(defaultListId);
      expect(document.getElementById(utilityListId!)).toHaveProperty("role", "listbox");
      expect(defaultPicker.getAttribute("aria-controls")).toBeNull();
      const row = within(screen.getByTestId("settings-model-row-deepseek-v4-pro"));
      expect(row.getByTestId("settings-default-deepseek-v4-pro")).toBeTruthy();
      expect(row.getByTestId("settings-utility-deepseek-v4-pro")).toBeTruthy();
    });

    it("toggles the arrow without stealing focus or writing, and closes on Tab in either direction", async () => {
      const { postMessage } = mount();
      const state = connectState({ models, defaultModelId: "deepseek-v4-pro" });
      await emitState(state);
      const picker = input("settings-default-model-input");
      const toggle = screen.getByRole("button", { name: en("models.default.title") });
      const clickArrow = () => {
        expect(fireEvent.mouseDown(toggle)).toBe(false);
        fireEvent.click(toggle);
      };
      expect(toggle.tabIndex).toBe(-1);
      clickArrow();
      expect(document.activeElement).toBe(picker);
      expect(toggle.getAttribute("aria-expanded")).toBe("true");
      expect(toggle.getAttribute("aria-controls")).toBe(picker.getAttribute("aria-controls"));
      expect(screen.getAllByRole("option").every((option) => option.tabIndex === -1)).toBe(true);
      expect(screen.queryByTestId("settings-default-option-gpt-5.4")).toBeNull();
      clickArrow();
      expect(screen.queryByRole("listbox")).toBeNull();
      expect(toggle.getAttribute("aria-expanded")).toBe("false");
      expect(document.activeElement).toBe(picker);
      expect(picker.value).toBe("deepseek-v4-pro · DeepSeek");
      clickArrow();
      fireEvent.change(picker, { target: { value: "no-match" } });
      expect(screen.getByText(en("models.rolePicker.none"))).toBeTruthy();
      clickArrow();
      clickArrow();
      expect(picker.value).toBe("");
      expect(screen.getAllByRole("option")).toHaveLength(2);
      for (const shiftKey of [false, true]) {
        const event = new KeyboardEvent("keydown", { key: "Tab", shiftKey, bubbles: true, cancelable: true });
        fireEvent(picker, event);
        expect(event.defaultPrevented).toBe(false);
        expect(screen.queryByRole("listbox")).toBeNull();
        clickArrow();
      }
      fireEvent.click(screen.getByTestId("settings-default-option-deepseek-v4-pro"));
      expect(screen.queryByRole("listbox")).toBeNull();
      clickArrow();
      fireEvent.mouseDown(document.body);
      expect(screen.queryByRole("listbox")).toBeNull();
      await emitState({ ...state, capabilities: { ...state.capabilities, setDefaultModel: false } });
      fireEvent.click(toggle);
      expect(screen.queryByRole("listbox")).toBeNull();
      expect(sent(postMessage, "setDefaultModel")).toHaveLength(0);
      expect(sent(postMessage, "setUtilityModel")).toHaveLength(0);
    });
  });

  it("carries a correlated Reload receipt through actual Settings state frames", async () => {
    const { postMessage } = mount();
    const source = { configKey: "key", name: "deepwiki", type: "mcp" as const, transport: "http" as const, source: "global" as const, state: "connected" as const, generation: "1", attempt: 1, recovery: null, toolCount: 2, resourceCount: 0, overridden: false, oauthConfigured: false };
    await emitState(readyState({ route: "connectors", connectors: [source] }));
    await emitDomAction({ kind: "clickTestId", testId: "connector-card-deepwiki" });
    await emitDomAction({ kind: "clickTestId", testId: "connector-reload" });
    expect(screen.getByTestId("connector-reload")).toHaveProperty("disabled", true);
    const request = postMessage.mock.calls.map(([message]) => message as SettingsIntent).find((message) => message.type === "reloadConnector")!;
    const receipt = { configKey: "key", requestId: request.messageId, generation: "2", phase: "accepted" as const };
    await emitState(readyState({ route: "connectors", connectors: [{ ...source, state: "connecting", generation: "2", recovery: { phase: "starting", maxAttempts: 3, remainingMs: 90_000 } }], connectorReloads: { key: receipt } }));
    expect(within(screen.getByRole("dialog")).getByText("Reconnecting… 1/3")).toBeTruthy();
    await emitState(readyState({ route: "connectors", connectors: [{ ...source, generation: "2" }], connectorReloads: { key: { ...receipt, phase: "succeeded" } } }));
    expect(screen.getByTestId("connector-reload")).toHaveProperty("disabled", false);
    expect(within(screen.getByRole("dialog")).getByText("Connector reconnected.")).toBeTruthy();
    expect(postMessage.mock.calls.filter(([message]) => message.type === "reloadConnector")).toHaveLength(1);
  });

  it("drives React-controlled connector form inputs through the DOM-action bridge", async () => {
    const { postMessage } = mount();
    await emitState(readyState({ connectors: [], route: "connectors" }));

    await emitDomAction({ kind: "clickTestId", testId: "connector-add-open" });
    await emitDomAction({ kind: "setInputValue", testId: "connector-name", value: "controlled" });
    await emitDomAction({ kind: "setInputValue", testId: "connector-command", value: "node" });
    await emitDomAction({ kind: "setInputValue", testId: "connector-args", value: "server.mjs" });
    await emitDomAction({ kind: "clickTestId", testId: "connector-add-submit" });

    expect(postMessage).toHaveBeenLastCalledWith(expect.objectContaining({
      data: expect.objectContaining({
        connector: expect.objectContaining({
          args: ["server.mjs"],
          command: "node",
          name: "controlled",
        }),
      }),
      type: "addConnector",
    }));
  });

  it("treats a saved connector with a deferred connection error as a successful add", async () => {
    const { postMessage } = mount();
    await emitState(readyState({ connectors: [], route: "connectors" }));

    await emitDomAction({ kind: "clickTestId", testId: "connector-add-open" });
    await emitDomAction({ kind: "setInputValue", testId: "connector-name", value: "saved-partial" });
    await emitDomAction({ kind: "setInputValue", testId: "connector-command", value: "node" });
    await emitDomAction({ kind: "clickTestId", testId: "connector-add-submit" });

    const addRequests = postMessage.mock.calls
      .map(([message]) => message as SettingsIntent)
      .filter(
        (message): message is Extract<SettingsIntent, { type: "addConnector" }> =>
          message.type === "addConnector",
      );
    const addRequest = addRequests[addRequests.length - 1];
    expect(addRequest).toBeDefined();

    await emitState(readyState({
      connectorReceipt: {
        configSaved: true,
        connectionStarted: false,
        error: "trust store is temporarily unavailable",
        name: "saved-partial",
        requestId: addRequest!.messageId,
      },
      connectors: [],
      route: "connectors",
      status: "Connector saved, but connection was not started. trust store is temporarily unavailable",
    }));

    expect(screen.queryByRole("dialog", { name: "Add Connector" })).toBeNull();
    expect(screen.getByText(/connector saved, but connection was not started/i)).toBeTruthy();
    expect(screen.queryByText("Unable to add connector.")).toBeNull();
  });

  it("renders warning banners pushed from the settings host", async () => {
    mount();

    await emitState(
      readyState({
        status: "Model saved.",
        warnings: [
          "API `openai-responses` expects reasoning effort, but thinking_format=`anthropic` will not send it.",
        ],
      }),
    );

    expect(
      screen.getByText(
        "API `openai-responses` expects reasoning effort, but thinking_format=`anthropic` will not send it.",
      ),
    ).toBeTruthy();
    expect(screen.getByText("Model saved.")).toBeTruthy();
  });

  it("shows extension and serve versions, and warns on missing or mismatched serve versions", async () => {
    mount();

    await emitState(
      readyState({
        expectedCliVersion: "0.1.20",
        extensionVersion: "0.1.24",
        serverVersion: "0.1.20",
      }),
    );

    expect(screen.getByTestId("settings-version-footer").textContent).toContain(
      "Extension v0.1.24",
    );
    expect(screen.getByTestId("settings-version-footer").textContent).toContain(
      "Serve v0.1.20",
    );
    expect(screen.queryByText(/did not report a version/i)).toBeNull();
    expect(screen.queryByText(/expects tomcat cli/i)).toBeNull();

    await emitState(
      readyState({
        expectedCliVersion: "0.1.20",
        extensionVersion: "0.1.24",
        serverVersion: null,
      }),
    );

    expect(screen.getByText(/did not report a version/i)).toBeTruthy();
    expect(screen.getByTestId("settings-version-footer").textContent).toContain(
      "Serve vunknown",
    );

    await emitState(
      readyState({
        expectedCliVersion: "0.1.20",
        extensionVersion: "0.1.24",
        serverVersion: "0.1.13",
      }),
    );

    expect(
      screen.getByText(
        "This extension expects tomcat CLI v0.1.20, but the connected serve reports v0.1.13. Rebuild or update the CLI binary, then restart serve.",
      ),
    ).toBeTruthy();
  });
});
