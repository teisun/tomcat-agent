import type { MessageKey } from "../../../src/shared/i18n";
import type { SettingsModelView } from "../../../src/shared/settingsProtocol";

export interface ProviderPreset {
  api: string;
  apiKeyEnv: string;
  baseUrl: string;
  labelKey: MessageKey;
  /** Also the prefix of new model IDs (`{provider}/{model_name}`). */
  provider: string;
  /** Absent means "follow the protocol default". */
  thinkingFormat?: string;
}

/**
 * Static on purpose: presets must not drift when the built-in catalog changes.
 * Recommended models are the built-ins whose key slot equals `apiKeyEnv`.
 */
export const PROVIDER_PRESETS: readonly ProviderPreset[] = [
  { api: "openai", apiKeyEnv: "DEEPSEEK_API_KEY", baseUrl: "https://api.deepseek.com", labelKey: "models.preset.deepseek", provider: "deepseek", thinkingFormat: "deepseek" },
  { api: "openai-responses", apiKeyEnv: "OPENAI_API_KEY", baseUrl: "https://api.openai.com/v1", labelKey: "models.preset.openai", provider: "openai", thinkingFormat: "openai" },
  { api: "anthropic-messages", apiKeyEnv: "ANTHROPIC_API_KEY", baseUrl: "https://api.anthropic.com", labelKey: "models.preset.anthropic", provider: "anthropic", thinkingFormat: "anthropic-adaptive" },
  { api: "openai", apiKeyEnv: "KIMI_API_KEY", baseUrl: "https://api.kimi.com/coding/v1", labelKey: "models.preset.kimi", provider: "kimi" },
  { api: "openai", apiKeyEnv: "ZHIPU_API_KEY", baseUrl: "https://open.bigmodel.cn/api/paas/v4", labelKey: "models.preset.zhipu", provider: "zhipu", thinkingFormat: "zai" },
  { api: "openai", apiKeyEnv: "MOONSHOT_API_KEY", baseUrl: "https://api.moonshot.cn/v1", labelKey: "models.preset.moonshot", provider: "moonshot", thinkingFormat: "doubao" },
  { api: "openai", apiKeyEnv: "MIMO_API_KEY", baseUrl: "https://token-plan-cn.xiaomimimo.com", labelKey: "models.preset.mimo", provider: "mimo", thinkingFormat: "doubao" },
];

const LABEL_OVERRIDES: Record<string, string> = {
  anthropic: "Anthropic",
  deepseek: "DeepSeek",
  mimo: "MiMo",
  moonshot: "Moonshot",
  openai: "OpenAI",
  zhipu: "Zhipu AI",
};

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

export function findPresetByKeyEnv(keyEnv: string): ProviderPreset | null {
  return PROVIDER_PRESETS.find((preset) => preset.apiKeyEnv === keyEnv.trim()) ?? null;
}

export function findMatchingProviderPreset(
  model: Pick<SettingsModelView, "api" | "baseUrl" | "provider">,
  presets: readonly ProviderPreset[] = PROVIDER_PRESETS,
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
