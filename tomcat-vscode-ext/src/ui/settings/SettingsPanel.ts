import fs from "node:fs";
import path from "node:path";

import * as vscode from "vscode";

import type { TomcatMessenger } from "../../serveClient/TomcatMessenger";
import {
  hasServeCapability,
  type InitializeResult,
  SERVE_CAPABILITY_LIST_MODELS,
  SERVE_CAPABILITY_LIST_PROVIDER_KEYS,
  SERVE_CAPABILITY_REMOVE_MODEL,
  SERVE_CAPABILITY_SET_PROVIDER_KEY,
  SERVE_CAPABILITY_UPSERT_MODEL,
} from "../../serveClient/initialize";
import type {
  ListModelsPayload,
  ListProviderKeysPayload,
  ModelEntryInput,
  ModelView as WireModelView,
  ProviderKeyView as WireProviderKeyView,
} from "../../serveClient/wire";
import type {
  SettingsCapabilities,
  SettingsHostFrame,
  SettingsIntent,
  SettingsModelCapabilities,
  SettingsModelInput,
  SettingsModelView,
  SettingsProviderKeyInput,
  SettingsProviderKeyView,
  SettingsRoute,
  SettingsStateSnapshot,
} from "../../shared/settingsProtocol";
import { isSettingsIntent as isSettingsIntentMessage } from "../../shared/settingsProtocol";
import { resolveWebviewEntryAssets } from "../guiAssets";

function getNonce(): string {
  return Math.random().toString(36).slice(2) + Math.random().toString(36).slice(2);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function parseCapabilities(value: unknown): SettingsModelCapabilities {
  if (!isRecord(value)) {
    return {
      files: false,
      reasoning: false,
      tools: false,
      vision: false,
      webSearch: false,
    };
  }
  return {
    files: value.files === true,
    reasoning: value.reasoning === true,
    tools: value.tools === true,
    vision: value.vision === true,
    webSearch: value.webSearch === true || value.web_search === true,
  };
}

function parseModelView(value: WireModelView): SettingsModelView {
  return {
    api: value.api,
    apiKeyEnv: value.apiKeyEnv,
    baseUrl: value.baseUrl ?? null,
    capabilities: parseCapabilities(value.capabilities),
    contextWindow: typeof value.contextWindow === "number" ? value.contextWindow : null,
    id: value.id,
    keyPresent: value.keyPresent === true,
    modelName: value.modelName ?? null,
    provider: value.provider,
    source: value.source === "user" ? "user" : "builtin",
    thinkingFormat: value.thinkingFormat ?? null,
  };
}

function parseProviderKeyView(value: WireProviderKeyView): SettingsProviderKeyView {
  return {
    envName: value.envName,
    keyPresent: value.keyPresent === true,
    modelIds: value.modelIds,
    provider: value.provider,
  };
}

function parseModelsPayload(payload: ListModelsPayload | undefined): SettingsModelView[] {
  return payload?.models?.map(parseModelView) ?? [];
}

function parseProviderKeysPayload(payload: ListProviderKeysPayload | undefined): SettingsProviderKeyView[] {
  return payload?.keys?.map(parseProviderKeyView) ?? [];
}

function toWireModelEntryInput(model: SettingsModelInput): ModelEntryInput {
  return {
    api: model.api,
    apiKeyEnv: model.apiKeyEnv ?? null,
    baseUrl: model.baseUrl ?? null,
    capabilities: {
      files: model.capabilities.files,
      reasoning: model.capabilities.reasoning,
      tools: model.capabilities.tools,
      vision: model.capabilities.vision,
      web_search: model.capabilities.webSearch,
    },
    contextWindow: model.contextWindow ?? null,
    id: model.id,
    modelName: model.modelName ?? null,
    provider: model.provider,
    thinkingFormat: model.thinkingFormat ?? null,
  };
}

export interface SettingsPanelDeps {
  ensureInitialized(): Promise<InitializeResult>;
  extensionUri: vscode.Uri;
  messenger: TomcatMessenger;
  onModelCatalogChanged?(): Promise<void> | void;
}

export class SettingsPanel implements vscode.Disposable {
  private panel?: vscode.WebviewPanel;
  private route: SettingsRoute = "models";
  private state: SettingsStateSnapshot = {
    capabilities: {
      listModels: false,
      listProviderKeys: false,
      removeModel: false,
      setProviderKey: false,
      upsertModel: false,
    },
    models: [],
    providerKeys: [],
    ready: false,
    route: "models",
  };

  constructor(private readonly deps: SettingsPanelDeps) {}

  dispose(): void {
    this.panel?.dispose();
    this.panel = undefined;
  }

  reveal(route: SettingsRoute = "models"): void {
    this.route = route;
    if (this.panel) {
      this.panel.reveal(vscode.ViewColumn.Active, true);
      void this.refreshState();
      return;
    }
    this.panel = vscode.window.createWebviewPanel(
      "tomcat.settings",
      "Tomcat Settings",
      {
        preserveFocus: true,
        viewColumn: vscode.ViewColumn.Active,
      },
      {
        enableScripts: true,
        localResourceRoots: [vscode.Uri.joinPath(this.deps.extensionUri, "gui", "dist")],
        retainContextWhenHidden: true,
      },
    );
    this.panel.onDidDispose(() => {
      this.panel = undefined;
    });
    this.panel.webview.onDidReceiveMessage((message: unknown) => {
      if (!isSettingsIntentMessage(message)) {
        return;
      }
      void this.handleIntent(message);
    });
    this.panel.webview.html = this.renderHtml(this.panel.webview);
    void this.refreshState();
  }

  __testingSnapshot(): {
    route: SettingsRoute;
    state: SettingsStateSnapshot;
    visible: boolean;
  } {
    return {
      route: this.route,
      state: JSON.parse(JSON.stringify(this.state)) as SettingsStateSnapshot,
      visible: Boolean(this.panel?.visible),
    };
  }

  async __testingDispatchIntent(intent: SettingsIntent): Promise<void> {
    await this.handleIntent(intent);
  }

  private async handleIntent(intent: SettingsIntent): Promise<void> {
    switch (intent.type) {
      case "settings.ready":
        this.route = intent.data?.route ?? this.route;
        await this.refreshState();
        return;
      case "listModels":
        await this.refreshState();
        return;
      case "upsertModel":
        await this.handleUpsertModel(intent.data.model, intent.data.providerKey);
        return;
      case "removeModel":
        await this.handleRemoveModel(intent.data.modelId);
        return;
      case "setProviderKey":
        await this.handleSetProviderKey(intent.data.envName, intent.data.value);
        return;
    }
  }

  private async handleUpsertModel(
    model: SettingsModelInput,
    providerKey?: SettingsProviderKeyInput,
  ): Promise<void> {
    try {
      const capabilities = this.buildCapabilities(await this.deps.ensureInitialized());
      if (!capabilities.upsertModel) {
        await this.refreshState("Model management is unavailable for this serve instance.");
        return;
      }
      const response = await this.deps.messenger.sendUpsertModel(toWireModelEntryInput(model));
      if (!response.success) {
        await this.refreshState(response.error ?? "Unable to save model.");
        return;
      }
      if (providerKey) {
        if (!capabilities.setProviderKey) {
          await this.refreshState(
            "Model saved, but this serve instance cannot store API keys yet.",
          );
          await this.deps.onModelCatalogChanged?.();
          return;
        }
        const keyResponse = await this.deps.messenger.sendSetProviderKey(
          providerKey.envName,
          providerKey.value,
        );
        if (!keyResponse.success) {
          await this.refreshState(
            `Model saved, but API key was not stored: ${keyResponse.error ?? "Unknown error."}`,
          );
          await this.deps.onModelCatalogChanged?.();
          return;
        }
        await this.refreshState(null, `Saved ${providerKey.envName}.`);
        await this.deps.onModelCatalogChanged?.();
        return;
      }
      await this.refreshState(null, "Model saved.");
      await this.deps.onModelCatalogChanged?.();
    } catch (error) {
      await this.refreshState(String(error), null);
    }
  }

  private async handleRemoveModel(modelId: string): Promise<void> {
    try {
      const capabilities = this.buildCapabilities(await this.deps.ensureInitialized());
      if (!capabilities.removeModel) {
        await this.refreshState("Model removal is unavailable for this serve instance.");
        return;
      }
      const response = await this.deps.messenger.sendRemoveModel(modelId);
      if (!response.success) {
        await this.refreshState(response.error ?? "Unable to remove model.");
        return;
      }
      await this.refreshState(null, "Model removed.");
      await this.deps.onModelCatalogChanged?.();
    } catch (error) {
      await this.refreshState(String(error), null);
    }
  }

  private async handleSetProviderKey(envName: string, value: string): Promise<void> {
    try {
      const capabilities = this.buildCapabilities(await this.deps.ensureInitialized());
      if (!capabilities.setProviderKey) {
        await this.refreshState("API key storage is unavailable for this serve instance.");
        return;
      }
      const response = await this.deps.messenger.sendSetProviderKey(envName, value);
      if (!response.success) {
        await this.refreshState(response.error ?? "Unable to store API key.");
        return;
      }
      await this.refreshState(null, `Saved ${envName}.`);
      await this.deps.onModelCatalogChanged?.();
    } catch (error) {
      await this.refreshState(String(error), null);
    }
  }

  private async refreshState(error: string | null = null, status: string | null = null): Promise<void> {
    const initializeResult = await this.deps.ensureInitialized();
    const capabilities = this.buildCapabilities(initializeResult);
    const modelsResult = capabilities.listModels
      ? await this.fetchModels(this.state.models)
      : { error: null, models: [] };
    const providerKeysResult = capabilities.listProviderKeys
      ? await this.fetchProviderKeys(this.state.providerKeys)
      : { error: null, providerKeys: [] };
    this.state = {
      capabilities,
      error: error ?? modelsResult.error ?? providerKeysResult.error,
      models: modelsResult.models,
      providerKeys: providerKeysResult.providerKeys,
      ready: true,
      route: this.route,
      status,
    };
    this.postState();
  }

  private async fetchModels(
    fallback: SettingsModelView[],
  ): Promise<{ error: string | null; models: SettingsModelView[] }> {
    try {
      const response = await this.deps.messenger.sendListModels();
      if (!response.success) {
        return {
          error: response.error ?? "Unable to load models.",
          models: fallback,
        };
      }
      return {
        error: null,
        models: parseModelsPayload(response.payload),
      };
    } catch (error) {
      return {
        error: String(error),
        models: fallback,
      };
    }
  }

  private async fetchProviderKeys(
    fallback: SettingsProviderKeyView[],
  ): Promise<{ error: string | null; providerKeys: SettingsProviderKeyView[] }> {
    try {
      const response = await this.deps.messenger.sendListProviderKeys();
      if (!response.success) {
        return {
          error: response.error ?? "Unable to load provider keys.",
          providerKeys: fallback,
        };
      }
      return {
        error: null,
        providerKeys: parseProviderKeysPayload(response.payload),
      };
    } catch (error) {
      return {
        error: String(error),
        providerKeys: fallback,
      };
    }
  }

  private buildCapabilities(initializeResult: InitializeResult): SettingsCapabilities {
    return {
      listModels: hasServeCapability(initializeResult, SERVE_CAPABILITY_LIST_MODELS),
      listProviderKeys: hasServeCapability(
        initializeResult,
        SERVE_CAPABILITY_LIST_PROVIDER_KEYS,
      ),
      removeModel: hasServeCapability(initializeResult, SERVE_CAPABILITY_REMOVE_MODEL),
      setProviderKey: hasServeCapability(
        initializeResult,
        SERVE_CAPABILITY_SET_PROVIDER_KEY,
      ),
      upsertModel: hasServeCapability(initializeResult, SERVE_CAPABILITY_UPSERT_MODEL),
    };
  }

  private postState(): void {
    if (!this.panel) {
      return;
    }
    const frame: SettingsHostFrame = {
      channel: "state",
      content: this.state,
      messageId: `settings-state-${Date.now()}`,
    };
    void this.panel.webview.postMessage(frame);
  }

  private renderHtml(webview: vscode.Webview): string {
    const distRoot = path.join(this.deps.extensionUri.fsPath, "gui", "dist");
    const assets = resolveWebviewEntryAssets(distRoot, "settings.html", "settings.js");
    if (assets.scripts.length === 0) {
      return this.renderFallbackHtml(
        "Tomcat settings assets are missing. Run `npm run build` in `tomcat-vscode-ext` first.",
      );
    }
    const nonce = getNonce();
    const styleTags = assets.stylesheets
      .map(
        (file) =>
          `<link rel="stylesheet" href="${webview.asWebviewUri(vscode.Uri.file(file)).toString()}" />`,
      )
      .join("\n    ");
    const scriptTags = assets.scripts
      .map(
        (file) =>
          `<script nonce="${nonce}" type="module" src="${webview.asWebviewUri(vscode.Uri.file(file)).toString()}"></script>`,
      )
      .join("\n    ");
    return `<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta
      http-equiv="Content-Security-Policy"
      content="default-src 'none'; img-src ${webview.cspSource} data:; font-src ${webview.cspSource}; style-src ${webview.cspSource}; script-src 'nonce-${nonce}';"
    />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    ${styleTags}
    <title>Tomcat Settings</title>
  </head>
  <body>
    <div id="root"></div>
    ${scriptTags}
  </body>
</html>`;
  }

  private renderFallbackHtml(message: string): string {
    return `<!DOCTYPE html>
<html lang="en">
  <body>
    <pre>${message}</pre>
  </body>
</html>`;
  }
}
