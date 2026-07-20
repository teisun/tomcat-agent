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

function readyState(overrides: Partial<SettingsStateSnapshot> = {}): SettingsStateSnapshot {
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
  it(
    "posts a ready handshake, defaults to the official tab, and supports keyboard tab switching",
    async () => {
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
    expect(within(dialog).getByRole("tablist", { name: /add model mode/i })).toBeTruthy();
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
    expect(within(dialog).queryByRole("textbox", { name: /base url/i })).toBeNull();

    fireEvent.keyDown(officialTab, { key: "ArrowRight" });
    expect(relayTab.getAttribute("aria-selected")).toBe("true");
    expect(relayTab.tabIndex).toBe(0);
    expect(officialTab.tabIndex).toBe(-1);
    expect(within(dialog).getByRole("textbox", { name: /base url/i })).toBeTruthy();
    expect(within(dialog).queryByLabelText("Provider")).toBeNull();

    fireEvent.keyDown(relayTab, { key: "Home" });
    expect(officialTab.getAttribute("aria-selected")).toBe("true");
    expect(within(dialog).queryByRole("textbox", { name: /base url/i })).toBeNull();

    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
    },
    15000,
  );

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
    expect(within(dialog).getByRole("textbox", { name: /base url/i })).toBeTruthy();

    fireEvent.click(officialTab);
    expect(officialTab.getAttribute("aria-selected")).toBe("true");
    expect(within(dialog).getByRole("status").textContent).toContain(
      "No official provider presets are available",
    );
    expect(
      within(dialog).getByText(/switch to relay \/ custom endpoint/i),
    ).toBeTruthy();
    expect(
      (within(dialog).getByRole("button", { name: "Save Model" }) as HTMLButtonElement)
        .disabled,
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
        providerKeys: [providerKey({ envName: "ANTHROPIC_API_KEY", keyPresent: true })],
      }),
    );
    postMessage.mockClear();

    const dialog = openAddModelDialog();
    fireEvent.change(within(dialog).getByRole("textbox", { name: /model name/i }), {
      target: { value: "gpt-5.6" },
    });
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
          contextWindow: null,
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
    fireEvent.change(within(dialog).getByRole("textbox", { name: /base url/i }), {
      target: { value: "https://api.chatanywhere.tech/v1" },
    });
    fireEvent.change(within(dialog).getByRole("textbox", { name: /model name/i }), {
      target: { value: "gpt-5.4" },
    });

    expect(within(dialog).getByText("chatanywhere")).toBeTruthy();
    expect(within(dialog).getByText("CHATANYWHERE_OPENAI_API_KEY")).toBeTruthy();
    expect(within(dialog).getByText("chatanywhere/gpt-5.4")).toBeTruthy();

    fireEvent.change(getPasswordInput(dialog), {
      target: { value: "relay-secret" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: /advanced/i }));
    fireEvent.change(within(dialog).getByPlaceholderText("chatanywhere/gpt-5.4"), {
      target: { value: "custom-relay-id" },
    });
    fireEvent.change(within(dialog).getByLabelText(/thinking format/i), {
      target: { value: "deepseek" },
    });
    fireEvent.click(within(dialog).getByLabelText("Vision"));
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Model" }));

    expect(postMessage).toHaveBeenCalledTimes(1);
    expect(postMessage.mock.calls[0][0]).toMatchObject({
      data: {
        model: {
          api: "openai",
          apiKeyEnv: "CHATANYWHERE_OPENAI_API_KEY",
          baseUrl: "https://api.chatanywhere.tech/v1",
          capabilities: {
            files: false,
            reasoning: true,
            tools: true,
            vision: true,
            webSearch: false,
          },
          contextWindow: null,
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
      (within(dialog).getByRole("textbox", { name: /base url/i }) as HTMLInputElement).value,
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
    fireEvent.change(within(dialog).getByRole("textbox", { name: /model name/i }), {
      target: { value: "gpt-5.4" },
    });
    expect(
      within(dialog).getByText(/override the built-in model `gpt-5\.4`/i),
    ).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));

    dialog = openAddModelDialog();
    fireEvent.click(
      within(dialog).getByRole("tab", { name: /relay \/ custom endpoint/i }),
    );
    fireEvent.change(within(dialog).getByRole("textbox", { name: /base url/i }), {
      target: { value: "https://" },
    });
    fireEvent.change(within(dialog).getByRole("textbox", { name: /model name/i }), {
      target: { value: "gpt-5.4" },
    });
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
    fireEvent.change(within(dialog).getByRole("textbox", { name: /base url/i }), {
      target: { value: "https://api.chatanywhere.tech/v1" },
    });
    fireEvent.change(within(dialog).getByRole("textbox", { name: /model name/i }), {
      target: { value: "gpt-5.4" },
    });
    fireEvent.change(within(dialog).getByLabelText("Key slot"), {
      target: { value: "OPENAI_API_KEY" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Model" }));

    expect(postMessage).toHaveBeenCalledTimes(1);
    expect(postMessage.mock.calls[0][0]).toMatchObject({
      data: {
        model: {
          api: "openai",
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
        providerKeys: [providerKey({ envName: "CHATANYWHERE_API_KEY", keyPresent: false })],
      }),
    );
    postMessage.mockClear();

    const inlineInput = screen.getByPlaceholderText("Save CHATANYWHERE_API_KEY") as HTMLInputElement;
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
        providerKeys: [providerKey({ envName: "CHATANYWHERE_API_KEY", keyPresent: false })],
      }),
    );

    fireEvent.click(screen.getByRole("button", { name: /add model/i }));
    const dialog = screen.getByRole("dialog");
    const modalKeyInput = getPasswordInput(dialog);
    expect(modalKeyInput.type).toBe("password");
    expect(modalKeyInput.autocomplete).toBe("off");

    fireEvent.change(within(dialog).getByRole("textbox", { name: /model name/i }), {
      target: { value: "gpt-5.6" },
    });
    fireEvent.change(modalKeyInput, {
      target: { value: "secret-value" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Model" }));

    expect(
      within(dialog).getByText("当前后端不支持保存 API Key，请先升级 `tomcat serve`。"),
    ).toBeTruthy();
  });

  it("refreshes key slots, validates custom key slots, removes the advanced duplicate, and masks drafts on blur", async () => {
    const { postMessage } = mount();
    await emitState(readyState({ models: [builtinModel()] }));
    postMessage.mockClear();

    const dialog = openAddModelDialog();
    fireEvent.click(within(dialog).getByRole("button", { name: /refresh key slots/i }));
    expect(postMessage.mock.calls[0][0]).toMatchObject({ type: "listProviderKeys" });
    postMessage.mockClear();

    await emitState(
      readyState({
        models: [builtinModel()],
        providerKeys: [providerKey({ envName: "FCODEX_OPENAI_API_KEY", keyPresent: true })],
      }),
    );
    expect(within(dialog).getByText("Key slots refreshed.")).toBeTruthy();

    const keySlot = within(dialog).getByRole("combobox", { name: "Key slot" });
    fireEvent.focus(keySlot);
    expect(within(dialog).getByRole("option", { name: /FCODEX_OPENAI_API_KEY/i })).toBeTruthy();

    fireEvent.change(within(dialog).getByRole("textbox", { name: /model name/i }), {
      target: { value: "gpt-5.6" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: /advanced/i }));
    expect(within(dialog).queryByText("API key env override")).toBeNull();

    fireEvent.change(keySlot, { target: { value: "bad-key-slot" } });
    expect(within(dialog).getByText("Key slot must match ^[A-Z_][A-Z0-9_]*$.")).toBeTruthy();
    fireEvent.change(keySlot, { target: { value: "FCODEX_OPENAI_API_KEY" } });

    const keyInput = within(dialog).getByLabelText("API key") as HTMLInputElement;
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
      expect(field.firstElementChild?.classList.contains("tc-field__label-row")).toBe(true);
    }
  });

  it("exposes stable visible-control hooks for key-slot alignment checks", async () => {
    mount();
    await emitState(readyState({ models: [builtinModel()] }));

    const dialog = openAddModelDialog();
    const keySlotBox = within(dialog).getByTestId("settings-key-slot-box");
    const apiKeyInput = within(dialog).getByTestId("settings-api-key-input");

    expect(keySlotBox.classList.contains("tc-settings-combobox")).toBe(true);
    expect(apiKeyInput.classList.contains("tc-settings-api-key-input")).toBe(true);
  });

  it("requires confirmation before replacing a configured key shared by other models", async () => {
    const { postMessage } = mount();
    await emitState(
      readyState({
        models: [
          builtinModel({ keyPresent: true }),
          builtinModel({ id: "gpt-4.1", keyPresent: true, modelName: "gpt-4.1" }),
        ],
        providerKeys: [providerKey({ keyPresent: true })],
      }),
    );
    postMessage.mockClear();

    const dialog = openAddModelDialog();
    fireEvent.change(within(dialog).getByRole("textbox", { name: /model name/i }), {
      target: { value: "gpt-5.6" },
    });
    const keyInput = within(dialog).getByLabelText("API key");
    fireEvent.focus(keyInput);
    fireEvent.change(keyInput, { target: { value: "rotated-secret" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Model" }));

    expect(postMessage).not.toHaveBeenCalled();
    const confirmation = screen.getByRole("alertdialog");
    expect(within(confirmation).getByText("gpt-5.4")).toBeTruthy();
    expect(within(confirmation).getByText("gpt-4.1")).toBeTruthy();
    fireEvent.click(within(confirmation).getByRole("button", { name: /replace shared key/i }));
    expect(postMessage).toHaveBeenCalledTimes(1);
    expect(postMessage.mock.calls[0][0]).toMatchObject({
      data: {
        model: { apiKeyEnv: "OPENAI_API_KEY", id: "gpt-5.6" },
        providerKey: { envName: "OPENAI_API_KEY", value: "rotated-secret" },
      },
      type: "upsertModel",
    });
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
        expectedCliVersion: "0.1.15",
        extensionVersion: "0.1.18",
        serverVersion: "0.1.15",
      }),
    );

    expect(screen.getByTestId("settings-version-footer").textContent).toContain(
      "Extension v0.1.18",
    );
    expect(screen.getByTestId("settings-version-footer").textContent).toContain(
      "Serve v0.1.15",
    );
    expect(screen.queryByText(/did not report a version/i)).toBeNull();
    expect(screen.queryByText(/expects tomcat cli/i)).toBeNull();

    await emitState(
      readyState({
        expectedCliVersion: "0.1.15",
        extensionVersion: "0.1.18",
        serverVersion: null,
      }),
    );

    expect(screen.getByText(/did not report a version/i)).toBeTruthy();
    expect(screen.getByTestId("settings-version-footer").textContent).toContain(
      "Serve vunknown",
    );

    await emitState(
      readyState({
        expectedCliVersion: "0.1.15",
        extensionVersion: "0.1.18",
        serverVersion: "0.1.13",
      }),
    );

    expect(
      screen.getByText(
        "This extension expects tomcat CLI v0.1.15, but the connected serve reports v0.1.13. Rebuild or update the CLI binary, then restart serve.",
      ),
    ).toBeTruthy();
  });
});
