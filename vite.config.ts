import { fileURLToPath } from "node:url";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// 完全本地：不使用任何 CDN、在线字体、远程脚本。
// 打包目标是本机 WebView（macOS: WKWebView），因此可以把编译目标放高。
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**", "**/docs/**"],
    },
  },
  build: {
    target: "es2022",
    sourcemap: false,
    emptyOutDir: true,
  },
});
