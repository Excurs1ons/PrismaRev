import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [vue()],

  // Vite 配置针对 Tauri dev/build 调整：
  // 1. 不清屏，避免遮挡 Rust 编译错误
  clearScreen: false,
  // 2. Tauri 需要固定端口
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. 不监听 src-tauri
      ignored: ["**/src-tauri/**"],
    },
  },
}));
