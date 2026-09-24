import type { JSX } from 'solid-js';

import { APP_NAME } from './app-info';
import { type BackendStatus, createBackendStatus } from './stores/backend';

/**
 * The bare window for Phase 0. The real shell — left rail, toolbar, status bar
 * — is P1-13, and the repo/status UI is P0-12.
 *
 * The one thing it does talk to Rust about is `ping`, the sample command that
 * proves the generated IPC bindings round-trip (P0-04). P0-12 replaces this
 * line with the real repository state.
 *
 * Strings are hardcoded English until the `t()` scaffolding lands (P6-20).
 */
export function App(): JSX.Element {
  const backend = createBackendStatus();

  return (
    <main class="app">
      <h1 class="app__title">{APP_NAME}</h1>
      <p class="app__hint">No repository is open yet.</p>
      <p class="app__status">{backendMessage(backend())}</p>
    </main>
  );
}

function backendMessage(status: BackendStatus): string {
  switch (status) {
    case 'checking':
      return 'Git engine: connecting…';
    case 'connected':
      return 'Git engine connected.';
    case 'unavailable':
      return 'Git engine unavailable.';
  }
}
