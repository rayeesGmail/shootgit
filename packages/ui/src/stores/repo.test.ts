import type {
  CommandError,
  OpenedRepo,
  RepoChanged,
  RepoId,
  RepoWatchFailed,
  Status,
  StatusEntry,
} from '@shootgit/ipc-types';
import { createRoot } from 'solid-js';
import { describe, expect, it, vi } from 'vitest';

import { type Outcome, type RepoBackend, type RepoStore, createRepoStore } from './repo';

// ---- fixtures -------------------------------------------------------------------

function entry(path: string, overrides: Partial<StatusEntry> = {}): StatusEntry {
  return {
    path,
    old_path: null,
    index_status: 'untracked',
    worktree_status: 'untracked',
    is_conflicted: false,
    is_submodule: false,
    ...overrides,
  };
}

function status(id: RepoId, paths: string[], path = '/work/repo'): Status {
  return {
    repo: {
      id,
      path,
      head: { kind: 'branch', name: 'main', oid: '0123456789abcdef0123456789abcdef01234567' },
      upstream: null,
      ahead_behind: null,
    },
    entries: paths.map((p) => entry(p)),
  };
}

function opened(s: Status, watchError: OpenedRepo['watch_error'] = null): Outcome<OpenedRepo> {
  return { status: 'ok', data: { status: s, watch_error: watchError } };
}

function failed<T>(error: Partial<CommandError> & Pick<CommandError, 'kind'>): Outcome<T> {
  return { status: 'error', error: { message: 'it failed', fix: null, ...error } };
}

/** A promise with its resolve function, for answers a test hands out later. */
function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve: (value: T) => void = () => {};
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

/**
 * A backend whose commands are `vi.fn`s and whose events the test fires by
 * hand. Defaults: no recent repositories, the dialog is cancelled.
 */
function fakeBackend() {
  let changed: ((event: RepoChanged) => void) | null = null;
  let watchFailed: ((event: RepoWatchFailed) => void) | null = null;
  const unlisten = vi.fn();
  const backend = {
    pickFolder: vi.fn((): Promise<string | null> => Promise.resolve(null)),
    openRepo: vi.fn((_path: string): Promise<Outcome<OpenedRepo>> => Promise.reject(new Error('unset'))),
    getStatus: vi.fn((_id: RepoId): Promise<Outcome<Status>> => Promise.reject(new Error('unset'))),
    listRecentRepos: vi.fn(
      (): Promise<Outcome<string[]>> => Promise.resolve({ status: 'ok', data: [] }),
    ),
    onRepoChanged: vi.fn((cb: (event: RepoChanged) => void) => {
      changed = cb;
      return Promise.resolve(unlisten);
    }),
    onWatchFailed: vi.fn((cb: (event: RepoWatchFailed) => void) => {
      watchFailed = cb;
      return Promise.resolve(unlisten);
    }),
  } satisfies RepoBackend;
  return {
    backend,
    unlisten,
    fireChanged(event: RepoChanged): void {
      if (changed === null) throw new Error('nobody listens to repo-changed');
      changed(event);
    },
    fireWatchFailed(event: RepoWatchFailed): void {
      if (watchFailed === null) throw new Error('nobody listens to repo-watch-failed');
      watchFailed(event);
    },
  };
}

/** Creates the store inside a Solid root; `dispose` ends the root. */
function makeStore(backend: RepoBackend): { store: RepoStore; dispose: () => void } {
  return createRoot((dispose) => ({ store: createRepoStore(backend), dispose }));
}

// ---- tests ----------------------------------------------------------------------

describe('createRepoStore', () => {
  it('starts with nothing open and loads the recent repositories', async () => {
    const fake = fakeBackend();
    fake.backend.listRecentRepos.mockResolvedValue({ status: 'ok', data: ['/work/b', '/work/a'] });

    const { store, dispose } = makeStore(fake.backend);

    expect(store.state.status).toBeNull();
    expect(store.state.error).toBeNull();
    await vi.waitFor(() => {
      expect(store.state.recent).toEqual(['/work/b', '/work/a']);
    });
    dispose();
  });

  it('opens the folder picked in the dialog and shows its status', async () => {
    const fake = fakeBackend();
    fake.backend.pickFolder.mockResolvedValue('/work/repo');
    fake.backend.openRepo.mockResolvedValue(opened(status(7, ['new.txt'])));
    const { store, dispose } = makeStore(fake.backend);

    await store.openFromDialog();

    expect(fake.backend.openRepo).toHaveBeenCalledWith('/work/repo');
    expect(store.state.status?.repo.id).toBe(7);
    expect(store.state.status?.entries.map((e) => e.path)).toEqual(['new.txt']);
    expect(store.state.opening).toBe(false);
    // Opening records the repository as recent; the list is read again.
    expect(fake.backend.listRecentRepos).toHaveBeenCalledTimes(2);
    dispose();
  });

  it('does nothing when the dialog is cancelled', async () => {
    const fake = fakeBackend();
    const { store, dispose } = makeStore(fake.backend);

    await store.openFromDialog();

    expect(fake.backend.openRepo).not.toHaveBeenCalled();
    expect(store.state.status).toBeNull();
    dispose();
  });

  it('shows why a repository could not be opened, with the fix, and keeps what was open', async () => {
    const fake = fakeBackend();
    fake.backend.openRepo.mockResolvedValueOnce(opened(status(1, ['a.txt'])));
    const { store, dispose } = makeStore(fake.backend);
    await store.open('/work/repo');

    fake.backend.openRepo.mockResolvedValueOnce(
      failed({
        kind: 'dubious_ownership',
        message: 'git refuses to work in /srv/theirs because another user owns it.',
        fix: "git config --global --add safe.directory '/srv/theirs'",
      }),
    );
    await store.open('/srv/theirs');

    expect(store.state.error?.kind).toBe('dubious_ownership');
    expect(store.state.error?.fix).toBe("git config --global --add safe.directory '/srv/theirs'");
    expect(store.state.status?.repo.id).toBe(1);
    dispose();
  });

  it('turns a broken IPC bridge into an error instead of throwing', async () => {
    const fake = fakeBackend();
    fake.backend.openRepo.mockRejectedValue(new Error('no Tauri host'));
    const { store, dispose } = makeStore(fake.backend);

    await store.open('/work/repo');

    expect(store.state.error).toEqual({ kind: 'failed', message: 'no Tauri host', fix: null });
    expect(store.state.opening).toBe(false);
    dispose();
  });

  it('refreshes the status when the open repository changes on disk', async () => {
    const fake = fakeBackend();
    fake.backend.openRepo.mockResolvedValue(opened(status(3, ['a.txt'])));
    fake.backend.getStatus.mockResolvedValue({ status: 'ok', data: status(3, ['a.txt', 'b.txt']) });
    const { store, dispose } = makeStore(fake.backend);
    await store.open('/work/repo');

    fake.fireChanged({ repo_id: 3, kinds: ['status'] });

    expect(fake.backend.getStatus).toHaveBeenCalledWith(3);
    await vi.waitFor(() => {
      expect(store.state.status?.entries.map((e) => e.path)).toEqual(['a.txt', 'b.txt']);
    });
    dispose();
  });

  it('ignores changes to a repository that is not the open one', async () => {
    const fake = fakeBackend();
    fake.backend.openRepo.mockResolvedValue(opened(status(3, [])));
    const { store, dispose } = makeStore(fake.backend);
    await store.open('/work/repo');

    fake.fireChanged({ repo_id: 4, kinds: ['status'] });

    expect(fake.backend.getStatus).not.toHaveBeenCalled();
    dispose();
  });

  it('drops a refresh that a newer one superseded', async () => {
    const fake = fakeBackend();
    fake.backend.openRepo.mockResolvedValue(opened(status(3, ['open.txt'])));
    const older = deferred<Outcome<Status>>();
    const newer = deferred<Outcome<Status>>();
    fake.backend.getStatus.mockReturnValueOnce(older.promise).mockReturnValueOnce(newer.promise);
    const { store, dispose } = makeStore(fake.backend);
    await store.open('/work/repo');

    const first = store.refresh();
    const second = store.refresh();
    newer.resolve({ status: 'ok', data: status(3, ['newer.txt']) });
    await second;
    // The backend cancels the older read; if its answer still arrives, it
    // must not replace the newer one.
    older.resolve({ status: 'ok', data: status(3, ['older.txt']) });
    await first;

    expect(store.state.status?.entries.map((e) => e.path)).toEqual(['newer.txt']);
    expect(store.state.error).toBeNull();
    dispose();
  });

  it('does not show a cancelled refresh as an error', async () => {
    const fake = fakeBackend();
    fake.backend.openRepo.mockResolvedValue(opened(status(3, ['open.txt'])));
    fake.backend.getStatus.mockResolvedValue(failed({ kind: 'cancelled' }));
    const { store, dispose } = makeStore(fake.backend);
    await store.open('/work/repo');

    await store.refresh();

    expect(store.state.error).toBeNull();
    expect(store.state.status?.entries.map((e) => e.path)).toEqual(['open.txt']);
    dispose();
  });

  it('keeps only the answer for the repository opened last', async () => {
    const fake = fakeBackend();
    const slow = deferred<Outcome<OpenedRepo>>();
    fake.backend.openRepo
      .mockReturnValueOnce(slow.promise)
      .mockResolvedValueOnce(opened(status(2, ['second.txt'], '/work/second')));
    const { store, dispose } = makeStore(fake.backend);

    const first = store.open('/work/first');
    await store.open('/work/second');
    slow.resolve(opened(status(1, ['first.txt'], '/work/first')));
    await first;

    expect(store.state.status?.repo.id).toBe(2);
    dispose();
  });

  it('warns when the open repository is not being watched', async () => {
    const fake = fakeBackend();
    const failure = { message: 'the file watcher stopped', is_watch_limit: true, fix: 'sysctl …' };
    fake.backend.openRepo.mockResolvedValue(opened(status(5, [])));
    const { store, dispose } = makeStore(fake.backend);
    await store.open('/work/repo');
    expect(store.state.watchWarning).toBeNull();

    fake.fireWatchFailed({ repo_id: 6, error: failure });
    expect(store.state.watchWarning).toBeNull();
    fake.fireWatchFailed({ repo_id: 5, error: failure });
    expect(store.state.watchWarning).toEqual(failure);
    dispose();
  });

  it('warns when the watcher could not start at all', async () => {
    const fake = fakeBackend();
    const failure = { message: 'could not watch /work/repo', is_watch_limit: false, fix: null };
    fake.backend.openRepo.mockResolvedValue(opened(status(5, []), failure));
    const { store, dispose } = makeStore(fake.backend);

    await store.open('/work/repo');

    expect(store.state.watchWarning).toEqual(failure);
    dispose();
  });

  it('stops listening when disposed', async () => {
    const fake = fakeBackend();
    const { dispose } = makeStore(fake.backend);
    await vi.waitFor(() => {
      expect(fake.backend.onRepoChanged).toHaveBeenCalled();
      expect(fake.backend.onWatchFailed).toHaveBeenCalled();
    });

    dispose();

    await vi.waitFor(() => {
      expect(fake.unlisten).toHaveBeenCalledTimes(2);
    });
  });

  it('survives a backend that cannot listen at all', async () => {
    const fake = fakeBackend();
    fake.backend.onRepoChanged.mockRejectedValue(new Error('no Tauri host'));
    fake.backend.listRecentRepos.mockRejectedValue(new Error('no Tauri host'));

    const { store, dispose } = makeStore(fake.backend);

    await Promise.resolve();
    expect(store.state.recent).toEqual([]);
    dispose();
  });
});
