import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type {
  SettingsHostFrame,
  SettingsIntent,
  SettingsModelView,
  SettingsProviderKeyView,
  SettingsStateSnapshot,
  VsCodeApiLike,
} from "../../../src/shared/settingsProtocol";
import { SettingsApp } from "./SettingsApp";

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

function providerKey(
  overrides: Partial<SettingsProviderKeyView>,
): SettingsProviderKeyView {
  return {
    envName: "OPENAI_API_KEY",
    keyPresent: false,
    modelIds: ["gpt-5.4"],
    provider: "openai",
    ...overrides,
  };
}

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

function openAddModelDialog() {
  fireEvent.click(screen.getByRole("button", { name: /add model/i }));
  return screen.getByRole("dialog");
}

function getPasswordInput(scope: HTMLElement): HTMLInputElement {
  const input = scope.querySelector('input[type="password"]');
  if (!(input instanceof HTMLInputElement)) {
    throw new Error("Expected one password input in scope.");
  }
  return input;
}

describe("SettingsApp", () => {
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

  it("posts a ready handshake, defaults to the official tab, and supports keyboard tab switching", async () => {
    const { postMessage } = mount();

    expect(postMessage).toHaveBeenCalledTimes(1);
    expect(postMessage.mock.calls[0][0]).toMatchObject({
      data: { route: "models" },
      type: "settings.ready",
    });

    await emitState(
      readyState({
        models: [
          builtinModel(),
          builtinModel({
            api: "anthropic-messages",
            apiKeyEnv: "ANTHROPIC_API_KEY",
            baseUrl: "https://api.anthropic.com",
            capabilities: {
              files: false,
              reasoning: true,
              tools: true,
              vision: true,
              webSearch: false,
            },
            id: "claude-opus-4-8",
            keyPresent: true,
            modelName: "claude-opus-4-8",
            provider: "anthropic",
            thinkingFormat: "anthropic",
          }),
        ],
      }),
    );

    const dialog = openAddModelDialog();
    expect(
      within(dialog).getByRole("tablist", { name: /add model mode/i }),
    ).toBeTruthy();
    const officialTab = within(dialog).getByRole("tab", {
      name: /official new model/i,
    });
    const relayTab = within(dialog).getByRole("tab", {
      name: /relay \/ custom endpoint/i,
    });

    expect(officialTab.getAttribute("aria-selected")).toBe("true");
    expect(officialTab.tabIndex).toBe(0);
    expect(relayTab.tabIndex).toBe(-1);
    expect(within(dialog).getByLabelText("Provider")).toBeTruthy();
    expect(
      within(dialog).queryByRole("textbox", { name: /base url/i }),
    ).toBeNull();

    fireEvent.keyDown(officialTab, { key: "ArrowRight" });
    expect(relayTab.getAttribute("aria-selected")).toBe("true");
    expect(relayTab.tabIndex).toBe(0);
    expect(officialTab.tabIndex).toBe(-1);
    expect(
      within(dialog).getByRole("textbox", { name: /base url/i }),
    ).toBeTruthy();
    expect(within(dialog).queryByLabelText("Provider")).toBeNull();

    fireEvent.keyDown(relayTab, { key: "Home" });
    expect(officialTab.getAttribute("aria-selected")).toBe("true");
    expect(
      within(dialog).queryByRole("textbox", { name: /base url/i }),
    ).toBeNull();

    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
  }, 15000);

  it("falls back to the relay tab when no official presets are available and explains the empty state", async () => {
    mount();
    await emitState(
      readyState({
        models: [
          {
            ...builtinModel(),
            baseUrl: "https://gateway.example.test/v1",
            id: "openai/gpt-5.4",
            provider: "openai-gateway",
            source: "user",
          },
        ],
      }),
    );

    const dialog = openAddModelDialog();
    const officialTab = within(dialog).getByRole("tab", {
      name: /official new model/i,
    });
    const relayTab = within(dialog).getByRole("tab", {
      name: /relay \/ custom endpoint/i,
    });

    expect(relayTab.getAttribute("aria-selected")).toBe("true");
    expect(
      within(dialog).getByRole("textbox", { name: /base url/i }),
    ).toBeTruthy();

    fireEvent.click(officialTab);
    expect(officialTab.getAttribute("aria-selected")).toBe("true");
    expect(within(dialog).getByRole("status").textContent).toContain(
      "No official provider presets are available",
    );
    expect(
      within(dialog).getByText(/switch to relay \/ custom endpoint/i),
    ).toBeTruthy();
    expect(
      (
        within(dialog).getByRole("button", {
          name: "Save Model",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(true);
  });

  it("mode A saves a preset-backed model and stores a new key when the slot is missing", async () => {
    const { postMessage } = mount();
    await emitState(
      readyState({
        models: [
          builtinModel(),
          builtinModel({
            api: "anthropic-messages",
            apiKeyEnv: "ANTHROPIC_API_KEY",
            baseUrl: "https://api.anthropic.com",
            capabilities: {
              files: false,
              reasoning: true,
              tools: true,
              vision: true,
              webSearch: false,
            },
            id: "claude-opus-4-8",
            keyPresent: true,
            modelName: "claude-opus-4-8",
            provider: "anthropic",
            thinkingFormat: "anthropic",
          }),
        ],
        providerKeys: [
          providerKey({ envName: "ANTHROPIC_API_KEY", keyPresent: true }),
        ],
      }),
    );
    postMessage.mockClear();

    const dialog = openAddModelDialog();
    fireEvent.change(
      within(dialog).getByRole("textbox", { name: /model name/i }),
      {
        target: { value: "gpt-5.6" },
      },
    );
    fireEvent.change(getPasswordInput(dialog), {
      target: { value: "openai-secret" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Model" }));

    expect(postMessage).toHaveBeenCalledTimes(1);
    expect(postMessage.mock.calls[0][0]).toMatchObject({
      data: {
        model: {
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
          id: "gpt-5.6",
          modelName: "gpt-5.6",
          provider: "openai",
          thinkingFormat: "openai",
        },
        providerKey: {
          envName: "OPENAI_API_KEY",
          value: "openai-secret",
        },
      },
      type: "upsertModel",
    });
  });

  it("mode B derives provider env and id, and advanced overrides are saved", async () => {
    const { postMessage } = mount();
    await emitState(
      readyState({
        models: [builtinModel()],
        providerKeys: [providerKey({ keyPresent: true })],
      }),
    );
    postMessage.mockClear();

    const dialog = openAddModelDialog();
    fireEvent.click(
      within(dialog).getByRole("tab", { name: /relay \/ custom endpoint/i }),
    );
    fireEvent.change(
      within(dialog).getByRole("textbox", { name: /base url/i }),
      {
        target: { value: "https://api.chatanywhere.tech/v1" },
      },
    );
    fireEvent.change(
      within(dialog).getByRole("textbox", { name: /model name/i }),
      {
        target: { value: "gpt-5.4" },
      },
    );

    expect(within(dialog).getByText("chatanywhere")).toBeTruthy();
    expect(
      within(dialog).getByText("CHATANYWHERE_OPENAI_API_KEY"),
    ).toBeTruthy();
    expect(within(dialog).getByText("chatanywhere/gpt-5.4")).toBeTruthy();

    fireEvent.change(getPasswordInput(dialog), {
      target: { value: "relay-secret" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: /advanced/i }));
    fireEvent.change(
      within(dialog).getByLabelText("Model ID (alias)"),
      {
        target: { value: "custom-relay-id" },
      },
    );
    fireEvent.change(within(dialog).getByLabelText(/thinking format/i), {
      target: { value: "deepseek" },
    });
    fireEvent.click(within(dialog).getByLabelText("Vision"));
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Model" }));

    expect(postMessage).toHaveBeenCalledTimes(1);
    expect(postMessage.mock.calls[0][0]).toMatchObject({
      data: {
        model: {
          api: "openai-responses",
          apiKeyEnv: "CHATANYWHERE_OPENAI_API_KEY",
          baseUrl: "https://api.chatanywhere.tech/v1",
          capabilities: {
            files: true,
            reasoning: true,
            tools: true,
            vision: false,
            webSearch: false,
          },
          contextWindow: 400000,
          id: "custom-relay-id",
          modelName: "gpt-5.4",
          provider: "chatanywhere",
          thinkingFormat: "deepseek",
        },
        providerKey: {
          envName: "CHATANYWHERE_OPENAI_API_KEY",
          value: "relay-secret",
        },
      },
      type: "upsertModel",
    });
  });

  it("editing existing models falls back into the matching official or relay layout", async () => {
    mount();
    await emitState(
      readyState({
        models: [
          builtinModel({ keyPresent: true }),
          {
            ...builtinModel({
              baseUrl: "https://gateway.example.test/v1",
              id: "openai/gpt-5.4",
              keyPresent: true,
              source: "user",
            }),
          },
          {
            api: "openai",
            apiKeyEnv: "CHATANYWHERE_API_KEY",
            baseUrl: "https://api.chatanywhere.tech/v1",
            capabilities: {
              files: false,
              reasoning: true,
              tools: true,
              vision: false,
              webSearch: false,
            },
            contextWindow: null,
            id: "chatanywhere/gpt-5.4",
            keyPresent: false,
            modelName: "gpt-5.4",
            provider: "chatanywhere",
            source: "user",
            thinkingFormat: null,
          },
        ],
      }),
    );

    fireEvent.click(screen.getAllByRole("button", { name: "Edit" })[0]);
    let dialog = screen.getByRole("dialog");
    expect(
      within(dialog)
        .getByRole("tab", { name: /official new model/i })
        .getAttribute("aria-selected"),
    ).toBe("true");
    expect(within(dialog).getByLabelText("Provider")).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));

    fireEvent.click(screen.getAllByRole("button", { name: "Edit" })[1]);
    dialog = screen.getByRole("dialog");
    expect(
      within(dialog)
        .getByRole("tab", { name: /official new model/i })
        .getAttribute("aria-selected"),
    ).toBe("true");
    expect(within(dialog).getByLabelText("Provider")).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));

    fireEvent.click(screen.getAllByRole("button", { name: "Edit" })[2]);
    dialog = screen.getByRole("dialog");
    expect(
      within(dialog)
        .getByRole("tab", { name: /relay \/ custom endpoint/i })
        .getAttribute("aria-selected"),
    ).toBe("true");
    expect(
      (
        within(dialog).getByRole("textbox", {
          name: /base url/i,
        }) as HTMLInputElement
      ).value,
    ).toBe("https://api.chatanywhere.tech/v1");
    expect(within(dialog).queryByLabelText("Provider")).toBeNull();
  }, 15_000);

  it("warns on built-in id collisions, validates bad relay URLs, and can reuse configured key slots", async () => {
    const { postMessage } = mount();
    await emitState(
      readyState({
        models: [
          builtinModel({ keyPresent: true }),
          builtinModel({
            api: "anthropic-messages",
            apiKeyEnv: "ANTHROPIC_API_KEY",
            baseUrl: "https://api.anthropic.com",
            capabilities: {
              files: false,
              reasoning: true,
              tools: true,
              vision: true,
              webSearch: false,
            },
            id: "claude-opus-4-8",
            keyPresent: true,
            modelName: "claude-opus-4-8",
            provider: "anthropic",
            thinkingFormat: "anthropic",
          }),
        ],
        providerKeys: [providerKey({ keyPresent: true })],
      }),
    );
    postMessage.mockClear();

    let dialog = openAddModelDialog();
    fireEvent.change(
      within(dialog).getByRole("textbox", { name: /model name/i }),
      {
        target: { value: "gpt-5.4" },
      },
    );
    expect(
      within(dialog).getByText(/override the built-in model `gpt-5\.4`/i),
    ).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));

    dialog = openAddModelDialog();
    fireEvent.click(
      within(dialog).getByRole("tab", { name: /relay \/ custom endpoint/i }),
    );
    fireEvent.change(
      within(dialog).getByRole("textbox", { name: /base url/i }),
      {
        target: { value: "https://" },
      },
    );
    fireEvent.change(
      within(dialog).getByRole("textbox", { name: /model name/i }),
      {
        target: { value: "gpt-5.4" },
      },
    );
    fireEvent.change(getPasswordInput(dialog), {
      target: { value: "relay-secret" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Model" }));
    expect(
      within(dialog).getByText(
        "Base URL could not be parsed. Use a value like https://host/v1.",
      ),
    ).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));

    dialog = openAddModelDialog();
    fireEvent.click(
      within(dialog).getByRole("tab", { name: /relay \/ custom endpoint/i }),
    );
    fireEvent.change(
      within(dialog).getByRole("textbox", { name: /base url/i }),
      {
        target: { value: "https://api.chatanywhere.tech/v1" },
      },
    );
    fireEvent.change(
      within(dialog).getByRole("textbox", { name: /model name/i }),
      {
        target: { value: "gpt-5.4" },
      },
    );
    fireEvent.change(within(dialog).getByLabelText("Key slot"), {
      target: { value: "OPENAI_API_KEY" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Model" }));

    expect(postMessage).toHaveBeenCalledTimes(1);
    expect(postMessage.mock.calls[0][0]).toMatchObject({
      data: {
        model: {
          api: "openai-responses",
          apiKeyEnv: "OPENAI_API_KEY",
          baseUrl: "https://api.chatanywhere.tech/v1",
          id: "chatanywhere/gpt-5.4",
          modelName: "gpt-5.4",
          provider: "chatanywhere",
          thinkingFormat: "openai",
        },
        providerKey: undefined,
      },
      type: "upsertModel",
    });
  }, 15_000);

  it("sends inline key saves, keeps key fields masked, and shows the backend capability warning", async () => {
    const { postMessage } = mount();
    await emitState(
      readyState({
        models: [
          builtinModel({ keyPresent: false }),
          {
            api: "openai",
            apiKeyEnv: "CHATANYWHERE_API_KEY",
            baseUrl: "https://api.chatanywhere.tech/v1",
            capabilities: {
              files: false,
              reasoning: true,
              tools: true,
              vision: false,
              webSearch: false,
            },
            contextWindow: null,
            id: "chatanywhere/gpt-5.4",
            keyPresent: false,
            modelName: "gpt-5.4",
            provider: "chatanywhere",
            source: "user",
            thinkingFormat: null,
          },
        ],
        providerKeys: [
          providerKey({ envName: "CHATANYWHERE_API_KEY", keyPresent: false }),
        ],
      }),
    );
    postMessage.mockClear();

    const inlineInput = screen.getByPlaceholderText(
      "Save CHATANYWHERE_API_KEY",
    ) as HTMLInputElement;
    expect(inlineInput.type).toBe("password");
    expect(inlineInput.autocomplete).toBe("off");

    fireEvent.change(inlineInput, {
      target: { value: "relay-secret" },
    });
    fireEvent.click(
      within(inlineInput.parentElement as HTMLElement).getByRole("button", {
        name: "Save",
      }),
    );
    expect(postMessage).toHaveBeenCalledTimes(1);
    expect(postMessage.mock.calls[0][0]).toMatchObject({
      data: {
        envName: "CHATANYWHERE_API_KEY",
        value: "relay-secret",
      },
      type: "setProviderKey",
    });

    postMessage.mockClear();
    await emitState(
      readyState({
        capabilities: {
          listModels: true,
          listProviderKeys: true,
          removeModel: true,
          setProviderKey: false,
          upsertModel: true,
        },
        models: [
          builtinModel({ keyPresent: false }),
          {
            api: "openai",
            apiKeyEnv: "CHATANYWHERE_API_KEY",
            baseUrl: "https://api.chatanywhere.tech/v1",
            capabilities: {
              files: false,
              reasoning: true,
              tools: true,
              vision: false,
              webSearch: false,
            },
            contextWindow: null,
            id: "chatanywhere/gpt-5.4",
            keyPresent: false,
            modelName: "gpt-5.4",
            provider: "chatanywhere",
            source: "user",
            thinkingFormat: null,
          },
        ],
        providerKeys: [
          providerKey({ envName: "CHATANYWHERE_API_KEY", keyPresent: false }),
        ],
      }),
    );

    fireEvent.click(screen.getByRole("button", { name: /add model/i }));
    const dialog = screen.getByRole("dialog");
    const modalKeyInput = getPasswordInput(dialog);
    expect(modalKeyInput.type).toBe("password");
    expect(modalKeyInput.autocomplete).toBe("off");

    fireEvent.change(
      within(dialog).getByRole("textbox", { name: /model name/i }),
      {
        target: { value: "gpt-5.6" },
      },
    );
    fireEvent.change(modalKeyInput, {
      target: { value: "secret-value" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Model" }));

    expect(
      within(dialog).getByText(
        "当前后端不支持保存 API Key，请先升级 `tomcat serve`。",
      ),
    ).toBeTruthy();
  });

  it("refreshes key slots, validates custom key slots, removes the advanced duplicate, and masks drafts on blur", async () => {
    const { postMessage } = mount();
    await emitState(readyState({ models: [builtinModel()] }));
    postMessage.mockClear();

    const dialog = openAddModelDialog();
    fireEvent.click(
      within(dialog).getByRole("button", { name: /refresh key slots/i }),
    );
    expect(postMessage.mock.calls[0][0]).toMatchObject({
      type: "listProviderKeys",
    });
    postMessage.mockClear();

    await emitState(
      readyState({
        models: [builtinModel()],
        providerKeys: [
          providerKey({ envName: "FCODEX_OPENAI_API_KEY", keyPresent: true }),
        ],
      }),
    );
    expect(within(dialog).getByText("Key slots refreshed.")).toBeTruthy();

    const keySlot = within(dialog).getByRole("combobox", { name: "Key slot" });
    fireEvent.focus(keySlot);
    expect(
      within(dialog).getByRole("option", { name: /FCODEX_OPENAI_API_KEY/i }),
    ).toBeTruthy();

    fireEvent.change(
      within(dialog).getByRole("textbox", { name: /model name/i }),
      {
        target: { value: "gpt-5.6" },
      },
    );
    fireEvent.click(within(dialog).getByRole("button", { name: /advanced/i }));
    expect(within(dialog).queryByText("API key env override")).toBeNull();

    fireEvent.change(keySlot, { target: { value: "bad-key-slot" } });
    expect(
      within(dialog).getByText("Key slot must match ^[A-Z_][A-Z0-9_]*$."),
    ).toBeTruthy();
    fireEvent.change(keySlot, { target: { value: "FCODEX_OPENAI_API_KEY" } });

    const keyInput = within(dialog).getByLabelText(
      "API key",
    ) as HTMLInputElement;
    fireEvent.focus(keyInput);
    fireEvent.change(keyInput, { target: { value: "sk-1234567890abcdef" } });
    expect(keyInput.type).toBe("password");
    fireEvent.blur(keyInput);
    expect(keyInput.type).toBe("text");
    expect(keyInput.value).toMatch(/^sk-12345•+cdef$/);
    expect(keyInput.value).not.toContain("67890ab");
  });

  it("uses the same label-row structure for the key slot and api key fields", async () => {
    mount();
    await emitState(readyState({ models: [builtinModel()] }));

    const dialog = openAddModelDialog();
    const keySlot = within(dialog).getByRole("combobox", { name: "Key slot" });
    const sharedRow = keySlot.closest(".tc-settings-form__row");
    expect(sharedRow).toBeTruthy();

    const fieldChildren = Array.from(sharedRow?.children ?? []).filter(
      (node): node is HTMLElement =>
        node instanceof HTMLElement && node.classList.contains("tc-field"),
    );
    expect(fieldChildren).toHaveLength(2);

    for (const field of fieldChildren) {
      expect(
        field.firstElementChild?.classList.contains("tc-field__label-row"),
      ).toBe(true);
    }
  });

  it("exposes stable visible-control hooks for key-slot alignment checks", async () => {
    mount();
    await emitState(readyState({ models: [builtinModel()] }));

    const dialog = openAddModelDialog();
    const keySlotBox = within(dialog).getByTestId("settings-key-slot-box");
    const apiKeyInput = within(dialog).getByTestId("settings-api-key-input");

    expect(keySlotBox.classList.contains("tc-settings-combobox")).toBe(true);
    expect(apiKeyInput.classList.contains("tc-settings-api-key-input")).toBe(
      true,
    );
  });

  it("requires confirmation before replacing a configured key shared by other models", async () => {
    const { postMessage } = mount();
    await emitState(
      readyState({
        models: [
          builtinModel({ keyPresent: true }),
          builtinModel({
            id: "gpt-4.1",
            keyPresent: true,
            modelName: "gpt-4.1",
          }),
        ],
        providerKeys: [providerKey({ keyPresent: true })],
      }),
    );
    postMessage.mockClear();

    const dialog = openAddModelDialog();
    fireEvent.change(
      within(dialog).getByRole("textbox", { name: /model name/i }),
      {
        target: { value: "gpt-5.6" },
      },
    );
    const keyInput = within(dialog).getByLabelText("API key");
    fireEvent.focus(keyInput);
    fireEvent.change(keyInput, { target: { value: "rotated-secret" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Model" }));

    expect(postMessage).not.toHaveBeenCalled();
    const confirmation = screen.getByRole("alertdialog");
    expect(within(confirmation).getByText("gpt-5.4")).toBeTruthy();
    expect(within(confirmation).getByText("gpt-4.1")).toBeTruthy();
    fireEvent.click(
      within(confirmation).getByRole("button", { name: /replace shared key/i }),
    );
    expect(postMessage).toHaveBeenCalledTimes(1);
    expect(postMessage.mock.calls[0][0]).toMatchObject({
      data: {
        model: { apiKeyEnv: "OPENAI_API_KEY", id: "gpt-5.6" },
        providerKey: { envName: "OPENAI_API_KEY", value: "rotated-secret" },
      },
      type: "upsertModel",
    });
  });

  it("confirms model deletion, ignores a delayed receipt, and shows the real warning outcome", async () => {
    const { postMessage } = mount();
    const userModel = builtinModel({
      id: "relay/remove-me",
      keyPresent: true,
      modelName: "remove-me",
      source: "user",
    });
    await emitState(readyState({
      models: [builtinModel({ keyPresent: true }), userModel],
      providerKeys: [providerKey({ keyPresent: true })],
    }));
    postMessage.mockClear();

    fireEvent.click(screen.getAllByRole("button", { name: "Edit" })[1]);
    const form = screen.getByRole("dialog");
    fireEvent.click(within(form).getByRole("button", { name: "Delete" }));
    const confirmation = screen.getByRole("alertdialog");
    expect(within(confirmation).getByText("Delete relay/remove-me?")).toBeTruthy();
    expect(postMessage).not.toHaveBeenCalled();

    fireEvent.click(
      within(confirmation).getByRole("button", { name: "Delete model" }),
    );
    expect(postMessage).toHaveBeenCalledWith(expect.objectContaining({
      data: { modelId: "relay/remove-me" },
      type: "removeModel",
    }));
    expect(screen.getByRole("status").textContent).toContain("Deleting relay/remove-me");

    await emitState(readyState({
      modelRemovalReceipt: { modelId: "relay/other", success: true, warnings: [] },
      models: [builtinModel({ keyPresent: true }), userModel],
      providerKeys: [providerKey({ keyPresent: true })],
    }));
    expect(screen.getByRole("dialog")).toBeTruthy();
    expect(screen.getByRole("status").textContent).toContain("Deleting relay/remove-me");

    await emitState(readyState({
      modelRemovalReceipt: {
        modelId: "relay/remove-me",
        success: true,
        warnings: ["A later catalog refresh is still required."],
      },
      models: [builtinModel({ keyPresent: true })],
      providerKeys: [providerKey({ keyPresent: true })],
      status: "Model removed with warnings.",
      warnings: ["A later catalog refresh is still required."],
    }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByText("Model removed with warnings.")).toBeTruthy();
    expect(screen.getByText("A later catalog refresh is still required.")).toBeTruthy();
  });

  it("copies matching built-in metadata, but keeps only catalog limits read-only", async () => {
    const { postMessage } = mount();
    await emitState(
      readyState({
        models: [
          builtinModel({
            contextWindow: 400000,
            contextWindowOptions: [400000, 1000000],
            id: "gpt-5.6",
            keyPresent: true,
            maxOutputTokens: 32768,
            modelName: "gpt-5.6",
            supportedReasoningLevels: ["high", "xhigh"],
          }),
        ],
        providerKeys: [
          providerKey({
            envName: "CHATANYWHERE_OPENAI_API_KEY",
            keyPresent: true,
          }),
        ],
      }),
    );
    postMessage.mockClear();

    const dialog = openAddModelDialog();
    fireEvent.click(
      within(dialog).getByRole("tab", { name: /relay \/ custom endpoint/i }),
    );
    fireEvent.change(
      within(dialog).getByRole("textbox", { name: /base url/i }),
      {
        target: { value: "https://api.chatanywhere.tech/v1" },
      },
    );
    fireEvent.change(
      within(dialog).getByRole("textbox", { name: /model name/i }),
      {
        target: { value: "gpt-5.6" },
      },
    );
    await act(async () => {});

    expect(
      within(dialog).getByTestId("settings-builtin-capability-source")
        .textContent,
    ).toContain("built-in gpt-5.6");

    fireEvent.click(within(dialog).getByRole("button", { name: /advanced/i }));
    expect(
      within(dialog).getByTestId("settings-context-window-auto").textContent,
    ).toBe("400000");
    expect(
      within(dialog).queryByTestId("settings-context-window-input"),
    ).toBeNull();
    expect(
      within(dialog).getByTestId("settings-max-output-tokens-auto").textContent,
    ).toBe("32768");
    expect(
      within(dialog).queryByTestId("settings-max-output-tokens-input"),
    ).toBeNull();
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Model" }));

    expect(postMessage).toHaveBeenCalledTimes(1);
    expect(postMessage.mock.calls[0][0]).toMatchObject({
      data: {
        model: {
          contextWindow: 400000,
          contextWindowOptions: [400000, 1000000],
          id: "chatanywhere/gpt-5.6",
          maxOutputTokens: 32768,
          supportedReasoningLevels: ["high", "xhigh"],
        },
      },
      type: "upsertModel",
    });
  });

  it("copies a matched 1M Claude tier into a new relay without manual configuration", async () => {
    const { postMessage } = mount();
    await emitState(
      readyState({
        models: [
          builtinModel({
            api: "anthropic-messages",
            apiKeyEnv: "ANTHROPIC_API_KEY",
            contextWindow: 1_000_000,
            contextWindowOptions: [400_000, 1_000_000],
            id: "claude-opus-5",
            keyPresent: true,
            maxOutputTokens: 128_000,
            modelName: "claude-opus-5",
            provider: "anthropic",
            supportedReasoningLevels: ["low", "medium", "high", "xhigh", "max"],
            thinkingFormat: "anthropic-adaptive",
          }),
        ],
        providerKeys: [
          providerKey({
            envName: "FCODEX_ANTHROPIC_API_KEY",
            keyPresent: true,
          }),
        ],
      }),
    );
    postMessage.mockClear();

    const dialog = openAddModelDialog();
    fireEvent.click(
      within(dialog).getByRole("tab", { name: /relay \/ custom endpoint/i }),
    );
    fireEvent.change(
      within(dialog).getByRole("textbox", { name: /base url/i }),
      {
        target: { value: "https://fcodex.top" },
      },
    );
    fireEvent.change(
      within(dialog).getByRole("textbox", { name: /model name/i }),
      {
        target: { value: "claude-opus-5" },
      },
    );
    fireEvent.click(within(dialog).getByRole("button", { name: /advanced/i }));

    expect(
      within(dialog).getByTestId("settings-context-window-auto").textContent,
    ).toBe("1000000");
    expect(
      within(dialog).queryByTestId("settings-context-window-input"),
    ).toBeNull();

    fireEvent.change(within(dialog).getByLabelText("API key"), {
      target: { value: "new-relay-key" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Model" }));

    expect(postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        data: expect.objectContaining({
          model: expect.objectContaining({
            api: "anthropic-messages",
            capabilities: {
              files: false,
              reasoning: true,
              tools: true,
              vision: true,
              webSearch: false,
            },
            contextWindow: 1_000_000,
            contextWindowOptions: [400_000, 1_000_000],
            maxOutputTokens: 128_000,
            supportedReasoningLevels: ["low", "medium", "high", "xhigh", "max"],
            thinkingFormat: "anthropic-adaptive",
          }),
        }),
        type: "upsertModel",
      }),
    );
  });

  it("prefers a same-name models.toml entry over built-in metadata", async () => {
    const { postMessage } = mount();
    await emitState(
      readyState({
        models: [
          builtinModel({
            contextWindow: 400_000,
            id: "gpt-5.6",
            keyPresent: true,
            maxOutputTokens: 128_000,
            modelName: "gpt-5.6",
          }),
          builtinModel({
            capabilities: {
              files: false,
              reasoning: true,
              tools: true,
              vision: false,
              webSearch: true,
            },
            contextWindow: 1_000_000,
            contextWindowOptions: [400_000, 1_000_000],
            description: "Local gpt-5.6 relay profile",
            id: "local/gpt-5.6",
            keyPresent: true,
            maxOutputTokens: 64_000,
            modelName: "gpt-5.6",
            source: "user",
            supportedReasoningLevels: ["high"],
            thinkingFormat: "deepseek",
          }),
        ],
      }),
    );
    postMessage.mockClear();

    const dialog = openAddModelDialog();
    fireEvent.click(
      within(dialog).getByRole("tab", { name: /relay \/ custom endpoint/i }),
    );
    fireEvent.change(within(dialog).getByRole("textbox", { name: /base url/i }), {
      target: { value: "https://new-relay.example.test/v1" },
    });
    fireEvent.change(
      within(dialog).getByRole("textbox", { name: /model name/i }),
      { target: { value: "gpt-5.6" } },
    );
    await act(async () => {});

    expect(
      within(dialog).getByTestId("settings-builtin-capability-source").textContent,
    ).toContain("configured local/gpt-5.6");
    fireEvent.click(within(dialog).getByRole("button", { name: /advanced/i }));
    expect(
      within(dialog).getByTestId("settings-context-window-auto").textContent,
    ).toBe("1000000");
    expect(
      within(dialog).getByTestId("settings-max-output-tokens-auto").textContent,
    ).toBe("64000");
    fireEvent.change(
      within(dialog).getByRole("textbox", { name: "Description" }),
      { target: { value: "Edited local profile" } },
    );
    fireEvent.change(
      within(dialog).getByRole("textbox", {
        name: /supported effort levels/i,
      }),
      { target: { value: "low, high" } },
    );
    fireEvent.change(within(dialog).getByLabelText("API key"), {
      target: { value: "new-relay-key" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Model" }));

    expect(postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        data: expect.objectContaining({
          model: expect.objectContaining({
            capabilities: expect.objectContaining({
              vision: false,
              webSearch: true,
            }),
            contextWindow: 1_000_000,
            contextWindowOptions: [400_000, 1_000_000],
            description: "Edited local profile",
            maxOutputTokens: 64_000,
            supportedReasoningLevels: ["low", "high"],
            thinkingFormat: "deepseek",
          }),
        }),
        type: "upsertModel",
      }),
    );
  });

  it("allows editing a reused relay's provider and capabilities before saving", async () => {
    const { postMessage } = mount();
    await emitState(
      readyState({
        models: [
          builtinModel({
            id: "gpt-5.6",
            keyPresent: true,
            modelName: "gpt-5.6",
            provider: "openai",
          }),
        ],
      }),
    );
    postMessage.mockClear();

    const dialog = openAddModelDialog();
    fireEvent.click(
      within(dialog).getByRole("tab", { name: /relay \/ custom endpoint/i }),
    );
    fireEvent.change(within(dialog).getByRole("textbox", { name: /base url/i }), {
      target: { value: "https://relay.example.test/v1" },
    });
    fireEvent.change(
      within(dialog).getByRole("textbox", { name: /model name/i }),
      { target: { value: "gpt-5.6" } },
    );
    await act(async () => {});
    fireEvent.click(within(dialog).getByRole("button", { name: /advanced/i }));
    fireEvent.change(
      within(dialog).getByRole("textbox", { name: /provider override/i }),
      { target: { value: "my-relay" } },
    );
    fireEvent.click(within(dialog).getByLabelText("Vision"));
    fireEvent.change(within(dialog).getByLabelText("API key"), {
      target: { value: "test-relay-key" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Model" }));

    expect(postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        data: expect.objectContaining({
          model: expect.objectContaining({
            capabilities: expect.objectContaining({ vision: false }),
            provider: "my-relay",
          }),
        }),
        type: "upsertModel",
      }),
    );
  });

  it("reuses an exact configured relay endpoint before host heuristics", async () => {
    const { postMessage } = mount();
    await emitState(
      readyState({
        models: [
          builtinModel({
            apiKeyEnv: "MY_RELAY_KEY",
            baseUrl: "https://api.chatanywhere.tech/v1",
            id: "my-relay/existing",
            keyPresent: true,
            modelName: "existing",
            provider: "my-relay",
            source: "user",
          }),
        ],
        providerKeys: [
          providerKey({ envName: "MY_RELAY_KEY", keyPresent: true }),
        ],
      }),
    );
    postMessage.mockClear();

    const dialog = openAddModelDialog();
    fireEvent.click(
      within(dialog).getByRole("tab", { name: /relay \/ custom endpoint/i }),
    );
    fireEvent.change(within(dialog).getByRole("textbox", { name: /base url/i }), {
      target: { value: "https://api.chatanywhere.tech/v1/" },
    });
    fireEvent.change(
      within(dialog).getByRole("textbox", { name: /model name/i }),
      { target: { value: "new-upstream" } },
    );

    expect(within(dialog).getByText("my-relay")).toBeTruthy();
    expect(within(dialog).getByText("MY_RELAY_KEY")).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Model" }));

    expect(postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        data: expect.objectContaining({
          model: expect.objectContaining({
            apiKeyEnv: "MY_RELAY_KEY",
            provider: "my-relay",
          }),
        }),
        type: "upsertModel",
      }),
    );
  });

  it("uses the 400K default and never exposes a manual context field for an unmatched relay", async () => {
    mount();
    await emitState(
      readyState({
        models: [builtinModel({ keyPresent: true })],
        providerKeys: [
          providerKey({
            envName: "CHATANYWHERE_OPENAI_API_KEY",
            keyPresent: true,
          }),
        ],
      }),
    );

    const dialog = openAddModelDialog();
    fireEvent.click(
      within(dialog).getByRole("tab", { name: /relay \/ custom endpoint/i }),
    );
    fireEvent.change(
      within(dialog).getByRole("textbox", { name: /base url/i }),
      {
        target: { value: "https://api.chatanywhere.tech/v1" },
      },
    );
    fireEvent.change(
      within(dialog).getByRole("textbox", { name: /model name/i }),
      {
        target: { value: "custom-upstream" },
      },
    );
    fireEvent.click(within(dialog).getByRole("button", { name: /advanced/i }));

    expect(
      within(dialog).queryByTestId("settings-builtin-capability-source"),
    ).toBeNull();
    expect(
      within(dialog).getByTestId("settings-context-window-auto").textContent,
    ).toBe("400000");
    expect(
      within(dialog).queryByTestId("settings-context-window-input"),
    ).toBeNull();
    expect(
      within(dialog).getByTestId("settings-max-output-tokens-auto").textContent,
    ).toBe("128000");
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

  it("keeps an Add ID override including a deliberate clear, and locks the ID opened for Edit", async () => {
    const { postMessage } = mount();
    const editable = builtinModel({
      id: "relay/original-id",
      keyPresent: true,
      modelName: "original-model",
      source: "user",
    });
    await emitState(
      readyState({
        models: [builtinModel({ keyPresent: true }), editable],
        providerKeys: [providerKey({ keyPresent: true })],
      }),
    );
    postMessage.mockClear();

    let dialog = openAddModelDialog();
    const modelName = within(dialog).getByRole("textbox", { name: /model name/i });
    const id = within(dialog).getByLabelText("Model ID (alias)") as HTMLInputElement;
    fireEvent.change(modelName, { target: { value: "suggested-id" } });
    expect(id.value).toBe("suggested-id");
    fireEvent.change(id, { target: { value: "chosen-id" } });
    fireEvent.change(modelName, { target: { value: "another-suggestion" } });
    expect(id.value).toBe("chosen-id");

    fireEvent.click(
      within(dialog).getByRole("tab", { name: /relay \/ custom endpoint/i }),
    );
    fireEvent.click(
      within(dialog).getByRole("tab", { name: /official new model/i }),
    );
    expect(id.value).toBe("chosen-id");
    fireEvent.change(id, { target: { value: "" } });
    fireEvent.change(modelName, { target: { value: "third-suggestion" } });
    expect(id.value).toBe("");
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Model" }));
    expect(within(dialog).getByText("Model ID, provider, and API are all required.")).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));

    fireEvent.click(screen.getAllByRole("button", { name: "Edit" })[1]);
    dialog = screen.getByRole("dialog");
    const editId = within(dialog).getByLabelText("Model ID (alias)") as HTMLInputElement;
    expect(editId.value).toBe("relay/original-id");
    expect(editId.readOnly).toBe(true);
    fireEvent.change(editId, { target: { value: "must-not-change" } });
    expect(editId.value).toBe("relay/original-id");

    await emitState(
      readyState({
        models: [builtinModel({ keyPresent: true })],
        providerKeys: [providerKey({ keyPresent: true })],
      }),
    );
    expect(editId.value).toBe("relay/original-id");
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
