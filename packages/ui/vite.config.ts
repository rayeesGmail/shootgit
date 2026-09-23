import type { Options as SolidOptions } from 'vite-plugin-solid';
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

/**
 * Options for `vite-plugin-solid`, chosen from the Vite mode.
 *
 * The plugin injects solid-refresh, its hot-reload wrapper, whenever Vite runs
 * as a dev server outside production — it computes
 * `command === 'serve' && mode !== 'production' && hot !== false`
 * (vite-plugin-solid 2.11.14, `configResolved`). `vitest run` *is* a
 * `serve`-mode Vite in mode `test`, so the wrapper was being injected into the
 * test module graph as well. It imports the virtual module id
 * `/@solid-refresh`, which Vitest's Node runner turns into
 * `file:///@solid-refresh`; Windows rejects that as a filename
 * ("The argument 'filename' must be a file URL object, file URL string, or
 * absolute path string"), while POSIX accepts it as an absolute path. That is
 * why the suite died on Windows only.
 *
 * So HMR is switched off for the test mode and left alone everywhere else:
 * `pnpm tauri dev` still hot-reloads. `vite.config.test.ts` holds both halves
 * of that as assertions.
 */
export function solidPluginOptions(mode: string): SolidOptions {
  return { hot: mode !== 'test' };
}

export default defineConfig(({ mode }) => ({
  plugins: [solid(solidPluginOptions(mode))],
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
    include: ['src/**/*.test.{ts,tsx}', 'vite.config.test.ts'],
    // Explicit imports from 'vitest' instead of injected globals, so test
    // files typecheck under the same strict tsconfig as the app.
    globals: false,
  },
}));
