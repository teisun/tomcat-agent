import { normalizeLocale, setLocale } from "../../../src/shared/i18n";

// Runs before rendering/ready. Reuse each surface's existing state channel; no new handshake.
setLocale(normalizeLocale(document.documentElement.lang || "en"));
window.addEventListener("message", (event: MessageEvent<unknown>) => {
  if (!event.data || typeof event.data !== "object") return;
  const frame = event.data as { channel?: unknown; type?: unknown; content?: { locale?: unknown }; data?: { locale?: unknown } };
  const locale = frame.channel === "state" ? frame.content?.locale
    : frame.type === "preview.state" ? frame.data?.locale : undefined;
  if (locale === "en" || locale === "zh-CN") setLocale(locale);
});
