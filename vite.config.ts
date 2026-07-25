import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/target/**"] },
    proxy: {
      "/levelimposter-api": {
        target: "https://api.levelimposter.net",
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/levelimposter-api/u, ""),
      },
      "/levelimposter-search": {
        target: "https://T5IVXJGKB9-dsn.algolia.net",
        changeOrigin: true,
        rewrite: (path) =>
          path.replace(
            /^\/levelimposter-search/u,
            "/1/indexes/LevelImposter-Maps",
          ),
      },
    },
  },
});
