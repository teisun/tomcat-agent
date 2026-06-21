import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  build: {
    outDir: "dist",
    rollupOptions: {
      output: {
        assetFileNames: "[name][extname]",
        chunkFileNames: "chunks/[name].js",
        entryFileNames: "index.js",
      },
    },
  },
  test: {
    environment: "jsdom",
    globals: true,
  },
});
