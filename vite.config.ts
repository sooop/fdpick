import { defineConfig } from "vite";

// Tauri 2 개발 서버 설정. 포트는 src-tauri/tauri.conf.json 의 devUrl 과 일치해야 한다.
export default defineConfig({
  // Tauri CLI 로그가 지워지지 않도록 화면 지우기를 끈다.
  clearScreen: false,
  server: {
    port: 1420,
    // 포트가 점유되어 있으면 다른 포트로 넘어가지 않고 실패한다.
    strictPort: true,
    host: "127.0.0.1",
  },
  build: {
    outDir: "dist",
    // WebView2(Chromium 105 기준) 에서 동작하도록 타깃을 고정한다.
    target: "chrome105",
  },
});
