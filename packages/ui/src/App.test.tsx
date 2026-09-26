import type { InvokeArgs } from '@tauri-apps/api/core';
import { emit } from '@tauri-apps/api/event';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import type { CommandError, OpenedRepo, RepoChanged, Status } from '@shootgit/ipc-types';
import { render } from 'solid-js/web';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { App } from './App';

/**
 * P0-12: the window opens a repository through the native dialog and shows
 * its raw status, refreshed on `repo-changed`.
 *
 * These tests drive the whole frontend half of that: component → store →
 * `@shootgit/ipc-types` (generated from Rust) → `@tauri-apps/api` → the Tauri
 * IPC entry point, with only the last hop replaced by
 * `@tauri-apps/api/mocks`, the seam a real WebView plugs into. The dialog
 * plugin's `open` is an IPC call too (`plugin:dialog|open`). The Rust half is
 * covered by `src-tauri/tests/repos.rs`.
 */

afterEach(() => {
  clearMocks();
  document.body.innerHTML = '';
});

function renderApp(): { host: HTMLElement; dispose: () => void } {
  const host = document.createElement('div');
  document.body.append(host);
  return { host, dispose: render(() => <App />, host) };
}

const OID = '0123456789abcdef0123456789abcdef01234567';

function status(paths: string[]): Status {
  return {
    repo: {
      id: 9,
      path: '/work/my repo',
      head: { kind: 'branch', name: 'main', oid: OID },
      upstream: 'origin/main',
      ahead_behind: { ahead: 1, behind: 2 },
    },
    entries: paths.map((path) => ({
      path,
      old_path: null,
      index_status: 'untracked',
      worktree_status: 'untracked',
      is_conflicted: false,
      is_submodule: false,
    })),
  };
}

interface Backend {
  invoked: { cmd: string; args: InvokeArgs | undefined }[];
  /** What `get_status` answers next. */
  next: { status: Status };
}

/**
 * Mocks the Rust side: the dialog picks `/work/my repo`, `open_repo` answers
 * `opened`, and events are delivered in-process (`shouldMockEvents`). A
 * command that fails answers by throwing its `CommandError`, as Tauri
 * rejects the invoke with the error payload.
 */
function mockBackend(opened: OpenedRepo | CommandError, recent: string[] = []): Backend {
  const backend: Backend = { invoked: [], next: { status: status([]) } };
  mockIPC(
    (cmd, args) => {
      backend.invoked.push({ cmd, args });
      switch (cmd) {
        case 'list_recent_repos':
          return recent;
        case 'plugin:dialog|open':
          return '/work/my repo';
        case 'open_repo':
          if ('kind' in opened) {
            throw opened;
          }
          return opened;
        case 'get_status':
          return backend.next.status;
        default:
          return undefined;
      }
    },
    { shouldMockEvents: true },
  );
  return backend;
}

function commandsOf(backend: Backend): string[] {
  return backend.invoked.map((call) => call.cmd).filter((cmd) => !cmd.startsWith('plugin:event'));
}

function openButton(host: HTMLElement): HTMLButtonElement {
  const button = [...host.querySelectorAll('button')].find((b) =>
    b.textContent?.includes('Open repository'),
  );
  if (button === undefined) throw new Error('no "Open repository" button');
  return button;
}

describe('App', () => {
  it('renders the window heading and the open button before the backend answers', () => {
    // Nothing may wait on an IPC round-trip to paint (SPEC §4 Low-resource
    // operation, rule 9).
    mockIPC(() => new Promise<never>(() => {}), { shouldMockEvents: true });

    const { host, dispose } = renderApp();

    expect(host.querySelector('h1')?.textContent).toBe('Shootgit');
    expect(openButton(host).disabled).toBe(false);
    expect(host.textContent).toContain('No repository is open');
    dispose();
  });

  it('lists the recent repositories', async () => {
    mockBackend({ status: status([]), watch_error: null }, ['/work/newer', '/work/older']);

    const { host, dispose } = renderApp();

    await vi.waitFor(() => {
      expect(host.textContent).toContain('/work/newer');
      expect(host.textContent).toContain('/work/older');
    });
    dispose();
  });

  it('opens the folder picked in the native dialog and lists its raw status', async () => {
    const backend = mockBackend({ status: status(['new file.txt']), watch_error: null });
    const { host, dispose } = renderApp();

    openButton(host).click();

    await vi.waitFor(() => {
      expect(host.textContent).toContain('new file.txt');
    });
    expect(commandsOf(backend)).toEqual([
      'list_recent_repos',
      'plugin:dialog|open',
      'open_repo',
      'list_recent_repos',
    ]);
    const dialog = backend.invoked.find((call) => call.cmd === 'plugin:dialog|open');
    expect(dialog?.args).toMatchObject({ options: { directory: true, multiple: false } });
    const open = backend.invoked.find((call) => call.cmd === 'open_repo');
    expect(open?.args).toEqual({ path: '/work/my repo' });
    expect(host.textContent).toContain('/work/my repo');
    expect(host.textContent).toContain('main @ 0123456');
    expect(host.textContent).toContain('origin/main ↑1 ↓2');
    expect(host.querySelector('.status-list__code')?.textContent).toBe('??');
    dispose();
  });

  it('refreshes the list when the repository changes on disk', async () => {
    const backend = mockBackend({ status: status(['first.txt']), watch_error: null });
    const { host, dispose } = renderApp();
    openButton(host).click();
    await vi.waitFor(() => {
      expect(host.textContent).toContain('first.txt');
    });

    backend.next.status = status(['first.txt', 'second.txt']);
    const changed: RepoChanged = { repo_id: 9, kinds: ['status'] };
    await emit('repo-changed', changed);

    await vi.waitFor(() => {
      expect(host.textContent).toContain('second.txt');
    });
    const refresh = backend.invoked.find((call) => call.cmd === 'get_status');
    expect(refresh?.args).toEqual({ repoId: 9 });
    dispose();
  });

  it('shows why a repository could not be opened, with the command that fixes it', async () => {
    mockBackend({
      kind: 'dubious_ownership',
      message: 'git refuses to work in /work/my repo because another user owns it.',
      fix: "git config --global --add safe.directory '/work/my repo'",
    });
    const { host, dispose } = renderApp();

    openButton(host).click();

    await vi.waitFor(() => {
      expect(host.textContent).toContain('because another user owns it');
    });
    expect(host.querySelector('code')?.textContent).toBe(
      "git config --global --add safe.directory '/work/my repo'",
    );
    dispose();
  });

  it('warns when changes on disk are not being watched', async () => {
    mockBackend({
      status: status([]),
      watch_error: {
        message: 'could not watch /work/my repo',
        is_watch_limit: true,
        fix: 'sudo sysctl fs.inotify.max_user_watches=524288',
      },
    });
    const { host, dispose } = renderApp();

    openButton(host).click();

    await vi.waitFor(() => {
      expect(host.textContent).toContain('could not watch /work/my repo');
    });
    expect(host.querySelector('code')?.textContent).toBe(
      'sudo sysctl fs.inotify.max_user_watches=524288',
    );
    dispose();
  });
});
