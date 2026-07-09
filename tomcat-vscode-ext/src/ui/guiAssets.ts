import * as fs from "node:fs";
import * as path from "node:path";

export function resolveGuiStylesheet(distRoot: string): string | null {
  const canonicalPath = path.join(distRoot, "styles.css");
  if (fs.existsSync(canonicalPath)) {
    return canonicalPath;
  }

  try {
    const candidates = fs
      .readdirSync(distRoot, { withFileTypes: true })
      .filter((entry) => entry.isFile() && entry.name.endsWith(".css"))
      .map((entry) => path.join(distRoot, entry.name))
      .sort((left, right) => left.localeCompare(right));
    return candidates[0] ?? null;
  } catch {
    return null;
  }
}
