import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath, URL } from "node:url";

const here = (p: string) => fileURLToPath(new URL(p, import.meta.url));

// `@clappkit` is the shared front-end half: plain .ts inside the clappkit submodule, behind
// this alias, so there is no npm package and no new dependency. The submodule has no
// node_modules of its own, so its bare imports resolve by walking up into ours; the two
// aliases below say so outright, which keeps the shared hooks on the SAME React instance as
// the app rather than a second copy ("invalid hook call").
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  resolve: {
    alias: {
      "@clappkit": here("./clappkit/web/index.ts"),
      "@tauri-apps/api": here("./node_modules/@tauri-apps/api"),
      react: here("./node_modules/react"),
    },
  },
  server: { port: Number(process.env.PORT) || 1420, strictPort: true, fs: { allow: ["."] } },
  // dist-web, never dist: it is tauri.conf.json's frontendDist, and lib.sh reads it there.
  build: { target: "safari15", outDir: "dist-web", emptyOutDir: true },
});
