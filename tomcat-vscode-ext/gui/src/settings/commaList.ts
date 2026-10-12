/** Parse only when saving; input controls must retain their original draft. */
export function parseCommaList(value: string): string[] {
  return [...new Set(value.split(/[,，]/u).map((entry) => entry.trim()).filter(Boolean))];
}
