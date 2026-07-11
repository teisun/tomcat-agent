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

export function envNameForRelaySlug(slug: string): string {
  const normalized = slug.trim().toUpperCase().replace(/[^A-Z0-9]+/g, "_");
  return normalized ? `${normalized}_API_KEY` : "";
}

export function deriveRelayFields(
  baseUrl: string,
  modelName: string,
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
    apiKeyEnv: envNameForRelaySlug(slug),
    host,
    id: trimmedModelName ? `${slug}${separator}${trimmedModelName}` : "",
    provider: slug,
    slug,
  };
}
