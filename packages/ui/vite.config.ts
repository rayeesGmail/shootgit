import solid from 'vite-plugin-solid';
import { defineConfig } from 'vitest/config';

// Vite + Vitest config for the Tauri frontend.
//
// The dev server port is fixed because `src-tauri/tauri.conf.json` points
// `build.devUrl` at it; `strictPort` makes a port clash fail loudly instead of
// silently serving somewhere Tauri will not look.
//
// `build.target` is the oldest WebView we support per SPEC §9: WKWebView on
// macOS 12 (≈ Safari 15), WebView2 (evergreen Chromium) on Windows 10 1809+
// and WebKitGTK 4.1 on Linux. It is static on purpose: the build must not
// depend on which OS produced it.
export default defineConfig({
  plugins: [solid()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: ['safari15', 'chrome108'],
    // Tauri serves the bundle from a custom protocol; source maps stay out of
    // the shipped app to keep the download small (SPEC §1: < 25 MB).
    sourcemap: false,
  },
  test: {
    environment: 'jsdom',
    include: ['src/**/*.test.{ts,tsx}'],
    // Explicit imports from 'vitest' instead of injected globals, so test
    // files typecheck under the same strict tsconfig as the app.
    globals: false,
  },
});
