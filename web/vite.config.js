import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import { viteSingleFile } from "vite-plugin-singlefile";

// One self-contained index.html (no CDN, no asset files), embedded in the binary by build.rs.
export default defineConfig({
  plugins: [vue(), viteSingleFile()],
  base: "./",
  build: {
    assetsInlineLimit: 100000000,
    cssCodeSplit: false,
  },
  server: {
    port: 5175,
    proxy: {
      "/api": { target: "http://127.0.0.1:8160", ws: true },
      "/metrics": "http://127.0.0.1:8160",
      "/livez": "http://127.0.0.1:8160",
      "/readyz": "http://127.0.0.1:8160",
      "/x-nmos": "http://127.0.0.1:8160",
    },
  },
});
