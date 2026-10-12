/// <reference types="vitest/config" />
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// `pnpm dev` proxies the API to a daemon TCP listener given by OPENSIDE_DEV_API
// (for example http://localhost:PORT); production is always same-origin.
const devApi = process.env.OPENSIDE_DEV_API;

export default defineConfig({
  plugins: [react()],
  build: {
    outDir: "dist",
    // The daemon CSP forbids inline script; keep every asset an external file.
    assetsInlineLimit: 0,
    modulePreload: { polyfill: false },
    sourcemap: false,
  },
  server: devApi ? { proxy: { "/v1": { target: devApi, changeOrigin: false } } } : {},
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["src/test/setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
    // Node >= 25 enables a global localStorage that shadows jsdom's; tests need jsdom's.
    poolOptions: { forks: { execArgv: ["--no-experimental-webstorage"] } },
  },
});
