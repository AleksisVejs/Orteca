import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

// Tauri drives this dev server; fixed port, no auto-open.
export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  envPrefix: ["VITE_", "TAURI_"],
  build: { target: "chrome110", sourcemap: true },
});
