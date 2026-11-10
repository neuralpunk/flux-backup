import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { readFileSync } from "node:fs";

const pkg = JSON.parse(readFileSync(new URL("./package.json", import.meta.url), "utf8")) as { version: string };

// Tauri expects a fixed dev port and serves the built files from ./build.
export default defineConfig({
  plugins: [svelte(), tailwindcss()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  // The app's version, for the About dialog and the demo build.
  define: { __APP_VERSION__: JSON.stringify(pkg.version) },
  build: {
    outDir: "build",
    target: "es2022",
    sourcemap: false,
  },
});
