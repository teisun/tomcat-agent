import type { Translator } from "../../../src/shared/i18n";
import type { SettingsKeySource, SettingsModelView } from "../../../src/shared/settingsProtocol";
import { findPresetByKeyEnv, humanizeProviderLabel, PROVIDER_PRESETS, type ProviderPreset } from "./providerPresets";
import { extractHost } from "./relayDerive";

export interface ProviderCardRow {
  isDefault: boolean;
  isUtility: boolean;
  model: SettingsModelView;
}

/** One card per effective key slot; derived on every render, never persisted. */
export interface ProviderCard {
  /**
   * The row that speaks for the card's own address and provider: its first user
   * entry, since built-ins keep catalog order and a built-in moved onto a relay
   * key still points at its official host.
   */
  anchor: SettingsModelView;
  hosts: string[];
  keyEnv: string;
  keyPresent: boolean;
  keySource: SettingsKeySource | null;
  preset: ProviderPreset | null;
  rows: ProviderCardRow[];
  title: string;
}

export interface ProviderCardsResult {
  cards: ProviderCard[];
  /** Presets without a card, in preset order. */
  connectable: ProviderPreset[];
}

/** Length first so `FCODEX_API_KEY` < `FCODEX_2_API_KEY` and `_9_` < `_10_`. */
function compareSlots(left: string, right: string): number {
  return left.length - right.length || (left < right ? -1 : left > right ? 1 : 0);
}

export function buildProviderCards(input: {
  defaultModelId?: string;
  models: readonly SettingsModelView[];
  presetLabel(preset: ProviderPreset): string;
  utilityModelId?: string;
}): ProviderCardsResult {
  const groups = new Map<string, SettingsModelView[]>();
  for (const model of input.models) {
    const slot = model.apiKeyEnv.trim();
    const group = groups.get(slot);
    if (group) group.push(model);
    else groups.set(slot, [model]);
  }

  const untitled: Array<Omit<ProviderCard, "title"> & { baseTitle: string }> = [];
  for (const [keyEnv, models] of groups) {
    const keyPresent = models.some((model) => model.keyPresent);
    // A slot with only built-ins and no key is an unconnected provider, not a card.
    if (!keyPresent && models.every((model) => model.source === "builtin")) continue;
    const preset = findPresetByKeyEnv(keyEnv);
    const anchor = models.find((model) => model.source !== "builtin") ?? models[0];
    const hosts: string[] = [];
    for (const model of [anchor, ...models]) {
      const host = model.baseUrl ? extractHost(model.baseUrl) : "";
      if (host && !hosts.includes(host)) hosts.push(host);
    }
    untitled.push({
      anchor,
      baseTitle: preset ? input.presetLabel(preset) : humanizeProviderLabel(anchor.provider),
      hosts,
      keyEnv,
      keyPresent,
      keySource: keyPresent ? models.find((model) => model.keySource)?.keySource ?? null : null,
      preset,
      rows: models.map((model) => ({
        isDefault: model.id === input.defaultModelId,
        isUtility: model.id === input.utilityModelId,
        model,
      })),
    });
  }

  const titles = new Map<string, string>();
  const byBaseTitle = new Map<string, string[]>();
  for (const card of untitled) {
    byBaseTitle.set(card.baseTitle, [...(byBaseTitle.get(card.baseTitle) ?? []), card.keyEnv]);
  }
  for (const [baseTitle, slots] of byBaseTitle) {
    slots.sort(compareSlots).forEach((slot, index) => {
      titles.set(slot, index === 0 ? baseTitle : `${baseTitle} ${index + 1}`);
    });
  }

  const cards = untitled
    .map(({ baseTitle, ...card }) => ({ ...card, title: titles.get(card.keyEnv) ?? baseTitle }))
    .sort((left, right) =>
      Number(right.keyPresent) - Number(left.keyPresent)
      || left.title.localeCompare(right.title, undefined, { numeric: true }));
  const carded = new Set(cards.map((card) => card.keyEnv));
  return {
    cards,
    connectable: PROVIDER_PRESETS.filter((preset) => !carded.has(preset.apiKeyEnv)),
  };
}

/** The variable name is shown only when the user must remove it outside Tomcat. */
export function keyStatusLabel(card: ProviderCard, t: Translator): string {
  if (!card.keyPresent) return t("models.card.keyMissing");
  return card.keySource === "environment"
    ? t("models.card.keyEnv", { name: card.keyEnv })
    : t("models.card.keySaved");
}
