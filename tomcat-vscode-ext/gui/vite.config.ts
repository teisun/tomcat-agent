import path from "node:path";

import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  base: "./",
  define: {
    "import.meta.env.TOMCAT_CONTEXT_SEARCH_DEBOUNCE_MS": JSON.stringify(
      process.env.TOMCAT_CONTEXT_SEARCH_DEBOUNCE_MS ?? "",
    ),
  },
  plugins: [react()],
  build: {
    outDir: "dist",
    rollupOptions: {
      input: {
        index: path.resolve(__dirname, "index.html"),
        settings: path.resolve(__dirname, "settings.html"),
      },
      output: {
        assetFileNames: "[name][extname]",
        chunkFileNames: "chunks/[name].js",
        entryFileNames: "[name].js",
      },
    },
  },
  test: {
    environment: "jsdom",
    globals: true,
  },
});
