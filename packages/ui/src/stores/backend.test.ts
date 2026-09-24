import { createRoot } from 'solid-js';
import { describe, expect, it, vi } from 'vitest';

import { createBackendStatus } from './backend';

/**
 * Runs `body` inside a Solid root and disposes it afterwards, so the store's
 * signals live in an owner the way they do under a rendered component.
 */
async function inRoot<T>(body: () => Promise<T>): Promise<T> {
  return await new Promise<T>((resolve, reject) => {
    createRoot((dispose) => {
      body().then(
        (value) => {
          dispose();
          resolve(value);
        },
        (error: unknown) => {
          dispose();
          reject(error instanceof Error ? error : new Error(String(error)));
        },
      );
    });
  });
}

describe('createBackendStatus', () => {
  it('reports "checking" until the backend answers', async () => {
    await inRoot(async () => {
      // A ping that never settles: the status must not claim either outcome.
      const status = createBackendStatus(() => new Promise<string>(() => {}));
      expect(status()).toBe('checking');
      await Promise.resolve();
      expect(status()).toBe('checking');
    });
  });

  it('reports "connected" once the backend answers "pong"', async () => {
    await inRoot(async () => {
      const status = createBackendStatus(() => Promise.resolve('pong'));

      await vi.waitFor(() => {
        expect(status()).toBe('connected');
      });
    });
  });

  it('reports "unavailable" when the IPC bridge rejects', async () => {
    await inRoot(async () => {
      // What a WebView with no Tauri host does: `invoke` rejects.
      const status = createBackendStatus(() => Promise.reject(new Error('no IPC host')));

      await vi.waitFor(() => {
        expect(status()).toBe('unavailable');
      });
    });
  });

  it('reports "unavailable" when the answer is not the expected one', async () => {
    await inRoot(async () => {
      const status = createBackendStatus(() => Promise.resolve('something else'));

      await vi.waitFor(() => {
        expect(status()).toBe('unavailable');
      });
    });
  });

  it('asks the backend exactly once', async () => {
    await inRoot(async () => {
      const ping = vi.fn(() => Promise.resolve('pong'));
      const status = createBackendStatus(ping);

      await vi.waitFor(() => {
        expect(status()).toBe('connected');
      });
      // Reading the status repeatedly must not re-issue the command: a Tauri
      // round-trip is not free (SPEC §4 Low-resource operation).
      status();
      status();
      expect(ping).toHaveBeenCalledTimes(1);
    });
  });
});
