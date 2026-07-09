import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type {
  SettingsHostFrame,
  SettingsIntent,
  SettingsStateSnapshot,
  VsCodeApiLike,
} from "../../../src/shared/settingsProtocol";
import { SettingsApp } from "./SettingsApp";

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

describe("SettingsApp", () => {
  it("posts a ready handshake and saves models with inferred api key env names", async () => {
    const { postMessage } = mount();

    expect(postMessage).toHaveBeenCalledTimes(1);
    expect(postMessage.mock.calls[0][0]).toMatchObject({
      data: { route: "models" },
      type: "settings.ready",
    });

    await emitState(readyState());

    const dialog = openAddModelDialog();

    fireEvent.change(within(dialog).getByLabelText("Model ID"), {
      target: { value: "gateway-claude" },
    });
    fireEvent.change(within(dialog).getByLabelText("Provider"), {
      target: { value: "anthropic gateway" },
    });
    fireEvent.change(within(dialog).getByLabelText("API Key"), {
      target: { value: "secret-value" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Model" }));

    expect(postMessage).toHaveBeenCalledTimes(3);
    expect(postMessage.mock.calls[1][0]).toMatchObject({
      data: {
        model: {
          api: "openai",
          id: "gateway-claude",
          provider: "anthropic gateway",
        },
      },
      type: "upsertModel",
    });
    expect(postMessage.mock.calls[2][0]).toMatchObject({
      data: {
        envName: "ANTHROPIC_GATEWAY_API_KEY",
        value: "secret-value",
      },
      type: "setProviderKey",
    });
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("renders ready vs needs-key groups and sends inline key saves", async () => {
    const { postMessage } = mount();
    postMessage.mockClear();

    await emitState(
      readyState({
        models: [
          {
            api: "openai-responses",
            apiKeyEnv: "OPENAI_API_KEY",
            baseUrl: null,
            capabilities: {
              files: true,
              reasoning: true,
              tools: true,
              vision: true,
              webSearch: false,
            },
            contextWindow: null,
            id: "gpt-5.4",
            keyPresent: true,
            modelName: null,
            provider: "openai",
            source: "builtin",
            thinkingFormat: null,
          },
          {
            api: "anthropic-messages",
            apiKeyEnv: "CLAUDE_GATEWAY_KEY",
            baseUrl: "https://api.example.com/v1",
            capabilities: {
              files: false,
              reasoning: true,
              tools: true,
              vision: false,
              webSearch: false,
            },
            contextWindow: null,
            id: "claude-opus-gateway",
            keyPresent: false,
            modelName: "claude-opus-4-6",
            provider: "anthropic",
            source: "user",
            thinkingFormat: "anthropic",
          },
        ],
        providerKeys: [
          {
            envName: "OPENAI_API_KEY",
            keyPresent: true,
            modelIds: ["gpt-5.4"],
            provider: "openai",
          },
          {
            envName: "CLAUDE_GATEWAY_KEY",
            keyPresent: false,
            modelIds: ["claude-opus-gateway"],
            provider: "anthropic",
          },
        ],
      }),
    );

    expect(screen.getAllByText("Ready").length).toBeGreaterThan(0);
    expect(screen.getByText("Needs API Key")).toBeTruthy();
    expect(screen.getByText("gpt-5.4")).toBeTruthy();
    expect(screen.getByText("claude-opus-gateway")).toBeTruthy();
    expect(screen.queryByText("builtin · openai · openai")).toBeNull();
    expect(
      screen.getByRole("button", { name: "Show details for gpt-5.4" }),
    ).toBeTruthy();

    const inlineInput = screen.getByPlaceholderText("Save CLAUDE_GATEWAY_KEY");
    fireEvent.change(inlineInput, {
      target: { value: "relay-secret" },
    });
    fireEvent.click(within(inlineInput.parentElement as HTMLElement).getByRole("button", { name: "Save" }));

    expect(postMessage).toHaveBeenCalledTimes(1);
    expect(postMessage.mock.calls[0][0]).toMatchObject({
      data: {
        envName: "CLAUDE_GATEWAY_KEY",
        value: "relay-secret",
      },
      type: "setProviderKey",
    });
  });

  it("shows a visible validation error when api key saving is unsupported", async () => {
    const { postMessage } = mount();
    await emitState(
      readyState({
        capabilities: {
          listModels: true,
          listProviderKeys: true,
          removeModel: true,
          setProviderKey: false,
          upsertModel: true,
        },
      }),
    );

    const dialog = openAddModelDialog();

    fireEvent.change(within(dialog).getByLabelText("Model ID"), {
      target: { value: "gateway-claude" },
    });
    fireEvent.change(within(dialog).getByLabelText("Provider"), {
      target: { value: "anthropic gateway" },
    });
    fireEvent.change(within(dialog).getByLabelText("API Key"), {
      target: { value: "secret-value" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Model" }));

    expect(
      within(dialog).getByText("当前后端不支持保存 API Key，请先升级 `tomcat serve`。"),
    ).toBeTruthy();
    expect(postMessage).toHaveBeenCalledTimes(1);
  });

  it("locks model id while editing and keeps api key inputs as password fields", async () => {
    mount();
    await emitState(
      readyState({
        models: [
          {
            api: "anthropic-messages",
            apiKeyEnv: "CLAUDE_GATEWAY_KEY",
            baseUrl: "https://api.example.com/v1",
            capabilities: {
              files: false,
              reasoning: true,
              tools: true,
              vision: false,
              webSearch: false,
            },
            contextWindow: null,
            id: "claude-opus-gateway",
            keyPresent: false,
            modelName: "claude-opus-4-6",
            provider: "anthropic",
            source: "user",
            thinkingFormat: "anthropic",
          },
        ],
        providerKeys: [
          {
            envName: "CLAUDE_GATEWAY_KEY",
            keyPresent: false,
            modelIds: ["claude-opus-gateway"],
            provider: "anthropic",
          },
        ],
      }),
    );

    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    const dialog = screen.getByRole("dialog");

    const modelIdInput = within(dialog).getByLabelText("Model ID") as HTMLInputElement;
    expect(modelIdInput.disabled).toBe(true);

    const modalApiKeyInput = within(dialog).getByLabelText("API Key") as HTMLInputElement;
    expect(modalApiKeyInput.type).toBe("password");
    expect(modalApiKeyInput.autocomplete).toBe("off");

    const inlineInput = screen.getByPlaceholderText("Save CLAUDE_GATEWAY_KEY") as HTMLInputElement;
    expect(inlineInput.type).toBe("password");
    expect(inlineInput.autocomplete).toBe("off");
  });

  it("opens the modal from the header button and closes it on escape", async () => {
    mount();
    await emitState(readyState());

    openAddModelDialog();
    expect(screen.getByRole("dialog")).toBeTruthy();

    fireEvent.keyDown(window, { key: "Escape" });

    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
