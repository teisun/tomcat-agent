import type { SettingsModelView } from "../../../src/shared/settingsProtocol";

export const RELAY_ID_SEPARATOR = "/";

const KNOWN_SECOND_LEVEL_SUFFIXES = new Set([
  "co.jp",
  "co.kr",
  "co.nz",
  "co.uk",
  "com.au",
  "com.br",
  "com.cn",
  "com.hk",
  "com.sg",
  "com.tw",
]);

export interface RelayDerivedFields {
  apiKeyEnv: string;
  host: string;
  id: string;
  provider: string;
  slug: string;
}

function hasScheme(value: string): boolean {
  return /^[a-z][a-z0-9+.-]*:\/\//i.test(value);
}

function stripBrackets(host: string): string {
  return host.replace(/^\[|\]$/g, "");
}

function isIpv4(host: string): boolean {
  return /^\d{1,3}(?:\.\d{1,3}){3}$/.test(host);
}

function isIpv6(host: string): boolean {
  return host.includes(":");
}

function sanitizeBrand(value: string): string {
  return value.trim().toLowerCase().replace(/[^a-z0-9]+/g, "_").replace(/^_+|_+$/g, "");
}

function slugifyBrand(value: string): string {
  return value.trim().toLowerCase().replace(/[^a-z0-9]+/g, "");
}

function extractHost(baseUrl: string): string {
  const trimmed = baseUrl.trim();
  if (!trimmed) {
    return "";
  }

  const candidate = hasScheme(trimmed) ? trimmed : `https://${trimmed}`;
  try {
    return stripBrackets(new URL(candidate).hostname.toLowerCase());
  } catch {
    const withoutScheme = trimmed.replace(/^[a-z][a-z0-9+.-]*:\/\//i, "");
    const firstSegment = withoutScheme.split(/[/?#]/, 1)[0] ?? "";
    const withoutAuth = firstSegment.includes("@")
      ? firstSegment.slice(firstSegment.lastIndexOf("@") + 1)
      : firstSegment;
    if (!withoutAuth) {
      return "";
    }
    if (withoutAuth.includes(":") && !withoutAuth.includes("]")) {
      return stripBrackets(withoutAuth.toLowerCase());
    }
    return stripBrackets(withoutAuth.replace(/:\d+$/, "").toLowerCase());
  }
}

function pickBrand(host: string): string {
  const normalized = host.replace(/^(www|api)\./, "");
  if (!normalized) {
    return "";
  }
  if (normalized === "localhost" || isIpv4(normalized) || isIpv6(normalized)) {
    return sanitizeBrand(normalized);
  }

  const parts = normalized.split(".").filter(Boolean);
  if (parts.length === 0) {
    return "";
  }
  if (parts.length === 1) {
    return sanitizeBrand(parts[0]);
  }

  const suffix2 = parts.slice(-2).join(".");
  const label =
    KNOWN_SECOND_LEVEL_SUFFIXES.has(suffix2) && parts.length >= 3
      ? parts.at(-3) ?? ""
      : parts.at(-2) ?? parts[0];
  return sanitizeBrand(label);
}

export function envNameForRelaySlug(slug: string, api = "openai"): string {
  const normalized = slug.trim().toUpperCase().replace(/[^A-Z0-9]+/g, "_");
  if (!normalized) {
    return "";
  }
  const family = api.trim() === "anthropic-messages" ? "ANTHROPIC" : "OPENAI";
  return `${normalized}_${family}_API_KEY`;
}

/**
 * Canonicalizes user input solely for endpoint equality. This is deliberately
 * narrower than heuristic derivation: if parsing fails, it returns null so the
 * caller can fall back to deriveRelayFields instead of guessing a match.
 */
export function normalizeRelayEndpoint(baseUrl: string): string | null {
  const trimmed = baseUrl.trim();
  if (!trimmed) return null;
  const candidate = hasScheme(trimmed) ? trimmed : `https://${trimmed}`;
  try {
    const parsed = new URL(candidate);
    parsed.hash = "";
    parsed.search = "";
    return parsed.toString().replace(/\/+$/, "");
  } catch {
    return null;
  }
}

/**
 * Reuses credentials metadata only from an explicitly configured user relay.
 * Exact endpoint matching is deterministic and always takes precedence over
 * brand/host heuristics.
 */
export function findConfiguredRelayByBaseUrl(
  models: readonly SettingsModelView[],
  baseUrl: string,
): SettingsModelView | null {
  const target = normalizeRelayEndpoint(baseUrl);
  if (!target) return null;
  return (
    models.find(
      (model) =>
        model.source === "user" &&
        typeof model.baseUrl === "string" &&
        normalizeRelayEndpoint(model.baseUrl) === target,
    ) ?? null
  );
}

export function deriveRelayFields(
  baseUrl: string,
  modelName: string,
  api = "openai",
  separator = RELAY_ID_SEPARATOR,
): RelayDerivedFields {
  const trimmedBaseUrl = baseUrl.trim();
  if (!trimmedBaseUrl) {
    return {
      apiKeyEnv: "",
      host: "",
      id: "",
      provider: "",
      slug: "",
    };
  }

  const host = extractHost(trimmedBaseUrl);
  const brand = pickBrand(host);
  const slug = slugifyBrand(brand || "custom") || "custom";
  const trimmedModelName = modelName.trim();

  return {
    apiKeyEnv: envNameForRelaySlug(slug, api),
    host,
    id: trimmedModelName ? `${slug}${separator}${trimmedModelName}` : "",
    provider: slug,
    slug,
  };
}
