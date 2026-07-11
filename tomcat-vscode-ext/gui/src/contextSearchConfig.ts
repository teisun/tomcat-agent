const DEFAULT_CONTEXT_SEARCH_DEBOUNCE_MS = 150;

function parsePositiveInteger(value: string | undefined): number | undefined {
  if (!value) {
    return undefined;
  }
  const parsed = Number.parseInt(value.trim(), 10);
  return Number.isInteger(parsed) && parsed > 0 ? parsed : undefined;
}

export function readContextSearchDebounceMs(): number {
  return (
    parsePositiveInteger(import.meta.env.TOMCAT_CONTEXT_SEARCH_DEBOUNCE_MS)
    ?? DEFAULT_CONTEXT_SEARCH_DEBOUNCE_MS
  );
}
