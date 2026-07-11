import type {
  SettingsModelCapabilities,
  SettingsModelView,
} from "../../../src/shared/settingsProtocol";

export interface ProviderPreset {
  api: string;
  apiKeyEnv: string;
  baseUrl: string;
  capabilities: SettingsModelCapabilities;
  keyPresent: boolean;
  label: string;
  modelIds: string[];
  provider: string;
  thinkingFormat: string;
}

const LABEL_OVERRIDES: Record<string, string> = {
  anthropic: "Anthropic",
  deepseek: "DeepSeek",
  mimo: "MiMo",
  moonshot: "Moonshot",
  openai: "OpenAI",
  zhipu: "Zhipu AI",
};

function cloneCapabilities(
  capabilities: SettingsModelCapabilities,
): SettingsModelCapabilities {
  return { ...capabilities };
}

function titleCaseToken(token: string): string {
  if (!token) {
    return "";
  }
  return token[0].toUpperCase() + token.slice(1);
}

export function humanizeProviderLabel(provider: string): string {
  const trimmed = provider.trim().toLowerCase();
  if (!trimmed) {
    return "Custom";
  }
  if (LABEL_OVERRIDES[trimmed]) {
    return LABEL_OVERRIDES[trimmed];
  }
  return trimmed
    .split(/[^a-z0-9]+/i)
    .filter(Boolean)
    .map(titleCaseToken)
    .join(" ");
}

export function buildProviderPresets(
  models: SettingsModelView[],
): ProviderPreset[] {
  const grouped = new Map<string, ProviderPreset>();

  for (const model of models) {
    if (model.source !== "builtin") {
      continue;
    }

    const provider = model.provider.trim();
    if (!provider) {
      continue;
    }

    const existing = grouped.get(provider);
    if (!existing) {
      grouped.set(provider, {
        api: model.api,
        apiKeyEnv: model.apiKeyEnv?.trim() ?? "",
        baseUrl: model.baseUrl?.trim() ?? "",
        capabilities: cloneCapabilities(model.capabilities),
        keyPresent: model.keyPresent,
        label: humanizeProviderLabel(provider),
        modelIds: [model.id],
        provider,
        thinkingFormat: model.thinkingFormat?.trim() ?? "",
      });
      continue;
    }

    existing.keyPresent ||= model.keyPresent;
    existing.modelIds.push(model.id);
    if (!existing.apiKeyEnv && model.apiKeyEnv?.trim()) {
      existing.apiKeyEnv = model.apiKeyEnv.trim();
    }
    if (!existing.baseUrl && model.baseUrl?.trim()) {
      existing.baseUrl = model.baseUrl.trim();
    }
    if (!existing.thinkingFormat && model.thinkingFormat?.trim()) {
      existing.thinkingFormat = model.thinkingFormat.trim();
    }
  }

  return Array.from(grouped.values());
}

export function findMatchingProviderPreset(
  model: Pick<SettingsModelView, "api" | "baseUrl" | "provider">,
  presets: ProviderPreset[],
): ProviderPreset | null {
  const normalizedProvider = model.provider.trim();
  const normalizedApi = model.api.trim();
  const normalizedBaseUrl = model.baseUrl?.trim() ?? "";
  return (
    presets.find(
      (preset) =>
        preset.provider === normalizedProvider &&
        preset.api === normalizedApi &&
        preset.baseUrl === normalizedBaseUrl,
    ) ??
    presets.find(
      (preset) =>
        preset.provider === normalizedProvider && preset.api === normalizedApi,
    ) ?? null
  );
}
