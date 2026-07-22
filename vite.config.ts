import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import fs from "node:fs";
import path from "node:path";

// In an isolated worktree `node_modules` is a symlink into the shared repo, so
// pdf.js's `?url` worker import resolves to a path outside this project root and
// vite's fs allow-list denies it. Allow the real (deref'd) node_modules too. In
// the main checkout the symlink is absent and this resolves to the local dir.
const nodeModules = fs.realpathSync(path.resolve(__dirname, "node_modules"));

// Tauri expects a fixed port and ignores vite's HMR host guessing.
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: { "@": path.resolve(__dirname, "./src") },
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
    fs: { allow: [path.resolve(__dirname), nodeModules] },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "es2022",
    sourcemap: true,
  },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
