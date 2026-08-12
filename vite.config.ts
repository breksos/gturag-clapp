import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath, URL } from "node:url";

// `@clappkit` is the shared front-end half, consumed as plain .ts behind this alias —
// no npm package and no new dependency; React and @tauri-apps/api come from here.
// The submodule at the repo root is the one copy, so it cannot drift from the Rust side.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  resolve: {
    alias: {
      "@clappkit": fileURLToPath(new URL("./clappkit/web/index.ts", import.meta.url)),
    },
  },
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: "esnext",
    outDir: "dist",
    emptyOutDir: true,
  },
});
