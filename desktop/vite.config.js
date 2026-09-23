import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "path";
import { fileURLToPath } from "url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

/** `npm run mock`: loads the browser preview's Tauri/HTTP stand-ins ahead of the
 *  app. Only in `--mode mock`, so no other build ever references them. */
const mockPreview = {
  name: "airnote-mock-preview",
  transformIndexHtml: (html) =>
    html.replace(
      '<script type="module" src="/src/main.tsx"></script>',
      '<script type="module" src="/src/dev/mock/index.ts"></script>\n    $&',
    ),
};

export default defineConfig(({ mode }) => ({
  plugins: mode === "mock" ? [react(), mockPreview] : [react()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
  },
}));
