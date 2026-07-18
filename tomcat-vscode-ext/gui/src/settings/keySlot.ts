function fieldText(value: string | null | undefined): string {
  return typeof value === "string" ? value.trim() : "";
}

export function isValidKeySlotName(value: string | null | undefined): boolean {
  return /^[A-Z_][A-Z0-9_]*$/.test(fieldText(value));
}
