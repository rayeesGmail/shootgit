import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { render } from 'solid-js/web';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { App } from './App';

/**
 * P0-04's acceptance criterion: `ping() -> String` round-trips *from the UI*.
 *
 * These tests drive the whole frontend half of that trip — component →
 * `@shootgit/ipc-types` (generated from Rust) → `@tauri-apps/api`'s `invoke` →
 * the Tauri IPC entry point — with only the last hop replaced by
 * `@tauri-apps/api/mocks`, which is the same seam a real WebView plugs into.
 * The Rust half is covered by `src-tauri/tests/ipc.rs`; together they cover
 * the round-trip without needing a windowed app in CI.
 */

afterEach(() => {
  clearMocks();
  document.body.innerHTML = '';
});

/** Renders `<App />` into a detached host and returns it with its disposer. */
function renderApp(): { host: HTMLElement; dispose: () => void } {
  const host = document.createElement('div');
  document.body.append(host);
  return { host, dispose: render(() => <App />, host) };
}

describe('App', () => {
  it('invokes the Rust command named "ping" and shows that the engine answered', async () => {
    const invoked: string[] = [];
    mockIPC((cmd) => {
      invoked.push(cmd);
      return 'pong';
    });

    const { host, dispose } = renderApp();

    await vi.waitFor(() => {
      expect(host.textContent).toContain('Git engine connected');
    });
    // The command name is the contract between Rust and the generated
    // bindings; spelling it out here means a rename has to touch this test.
    expect(invoked).toEqual(['ping']);

    dispose();
  });

  it('says the engine is unavailable when the IPC bridge fails', async () => {
    mockIPC(() => {
      throw new Error('backend is not running');
    });

    const { host, dispose } = renderApp();

    await vi.waitFor(() => {
      expect(host.textContent).toContain('Git engine unavailable');
    });

    dispose();
  });

  it('renders the window heading before the backend has answered', async () => {
    // Nothing may wait on the IPC round-trip to paint (SPEC §4 Low-resource
    // operation, rule 9: nothing but settings and the last view at launch).
    mockIPC(() => new Promise<string>(() => {}));

    const { host, dispose } = renderApp();

    expect(host.querySelector('h1')?.textContent).toBe('Shootgit');
    await Promise.resolve();
    expect(host.textContent).toContain('Git engine');

    dispose();
  });
});
