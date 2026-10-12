import { defineConfig } from "vitest/config";
import base from "./vitest.config";

// Keep aliases and runtime settings shared; replace (do not concatenate) include.
export default defineConfig({
  ...base,
  test: { ...base.test, include: ["tests/**/*.test.ts"] },
});
