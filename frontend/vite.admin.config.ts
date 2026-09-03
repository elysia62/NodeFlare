import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  base: "/admin-assets/",
  plugins: [react()],
  publicDir: false,
  build: {
    outDir: "admin-dist",
    emptyOutDir: true,
    cssCodeSplit: false,
    rollupOptions: {
      input: "admin.html",
      output: {
        entryFileNames: "admin-[hash].js",
        chunkFileNames: "admin-[name]-[hash].js",
        assetFileNames: "admin-[hash][extname]",
      },
    },
  },
});
