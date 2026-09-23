import type { JSX } from 'solid-js';

import { APP_NAME } from './app-info';

/**
 * The bare window for Phase 0. The real shell — left rail, toolbar, status bar
 * — is P1-13, and the repo/status UI is P0-12; nothing here talks to the Rust
 * core yet.
 *
 * Strings are hardcoded English until the `t()` scaffolding lands (P6-20).
 */
export function App(): JSX.Element {
  return (
    <main class="app">
      <h1 class="app__title">{APP_NAME}</h1>
      <p class="app__hint">No repository is open yet.</p>
    </main>
  );
}
