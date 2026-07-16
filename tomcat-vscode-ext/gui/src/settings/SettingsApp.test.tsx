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
  });

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
  });

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
          thinkingFormat: null,
        },
        providerKey: undefined,
      },
      type: "upsertModel",
    });
  });

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

  it("refreshes models, validates custom key slots, removes the advanced duplicate, and masks drafts on blur", async () => {
    const { postMessage } = mount();
    await emitState(readyState({ models: [builtinModel()] }));
    postMessage.mockClear();

    fireEvent.click(screen.getByRole("button", { name: /refresh/i }));
    expect(postMessage.mock.calls[0][0]).toMatchObject({ type: "listModels" });
    postMessage.mockClear();

    const dialog = openAddModelDialog();
    fireEvent.change(within(dialog).getByRole("textbox", { name: /model name/i }), {
      target: { value: "gpt-5.6" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: /advanced/i }));
    expect(within(dialog).queryByText("API key env override")).toBeNull();

    const keySlot = within(dialog).getByRole("combobox", { name: "Key slot" });
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
});
