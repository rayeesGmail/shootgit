import {
  type CommandError,
  type OpenedRepo,
  type RepoChanged,
  type RepoId,
  type RepoWatchFailed,
  type Status,
  type WatchFailure,
  commands,
  events,
} from '@shootgit/ipc-types';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { onCleanup } from 'solid-js';
import { createStore } from 'solid-js/store';

/**
 * The open repository and what the window shows about it (P0-12): its
 * status, why the last action failed, and whether changes on disk are being
 * watched. One repository at a time in Phase 0.
 *
 * Everything reaches Rust through {@link RepoBackend}; the default is the
 * generated bindings plus the dialog plugin's folder picker, the only ways
 * the frontend may reach the backend (SPEC §4). Tests pass their own.
 */

/** What a generated command resolves to (`typedError` in the bindings). */
export type Outcome<T> = { status: 'ok'; data: T } | { status: 'error'; error: CommandError };

/** An unsubscribe function, as `listen` resolves to. */
export type Unlisten = () => void;

/** The backend calls the store makes. */
export interface RepoBackend {
  /** Shows the native folder picker; `null` when the user cancels. */
  pickFolder(): Promise<string | null>;
  openRepo(path: string): Promise<Outcome<OpenedRepo>>;
  getStatus(id: RepoId): Promise<Outcome<Status>>;
  listRecentRepos(): Promise<Outcome<string[]>>;
  onRepoChanged(listener: (event: RepoChanged) => void): Promise<Unlisten>;
  onWatchFailed(listener: (event: RepoWatchFailed) => void): Promise<Unlisten>;
}

/** The title of the folder picker. English until `t()` lands (P6-20). */
const PICKER_TITLE = 'Open repository';

/** The real backend: generated bindings and the dialog plugin. */
export const tauriBackend: RepoBackend = {
  pickFolder: () => openDialog({ directory: true, multiple: false, title: PICKER_TITLE }),
  openRepo: (path) => commands.openRepo(path),
  getStatus: (id) => commands.getStatus(id),
  listRecentRepos: () => commands.listRecentRepos(),
  onRepoChanged: (listener) => events.repoChanged.listen((event) => listener(event.payload)),
  onWatchFailed: (listener) => events.repoWatchFailed.listen((event) => listener(event.payload)),
};

/** The store's fields. Only the store writes them; readers get {@link RepoState}. */
interface RepoFields {
  /** Recently opened repositories, newest first. */
  recent: string[];
  /** An `open_repo` is in flight. */
  opening: boolean;
  /** The open repository's latest status; `null` until one is open. */
  status: Status | null;
  /** Why the last action failed, until the next open or a dismissal. */
  error: CommandError | null;
  /** Why the open repository is not being watched, if it is not. */
  watchWarning: WatchFailure | null;
}

export type RepoState = Readonly<RepoFields>;

export interface RepoStore {
  readonly state: RepoState;
  /** Asks for a folder with the native dialog and opens it. */
  openFromDialog(): Promise<void>;
  /** Opens the repository that contains `path`. */
  open(path: string): Promise<void>;
  /** Reads the open repository's status again. */
  refresh(): Promise<void>;
  dismissError(): void;
}

/** A failure that never reached a command: the IPC bridge itself threw. */
function bridgeError(error: unknown): CommandError {
  return {
    kind: 'failed',
    message: error instanceof Error ? error.message : String(error),
    fix: null,
  };
}

async function settle<T>(call: () => Promise<Outcome<T>>): Promise<Outcome<T>> {
  try {
    return await call();
  } catch (error) {
    return { status: 'error', error: bridgeError(error) };
  }
}

/**
 * Creates the store and subscribes to the backend's repository events for
 * as long as the calling Solid owner lives.
 *
 * Nothing waits on the backend to render (SPEC §4 Low-resource operation,
 * rule 9): the recent list arrives when it arrives. Only the newest answer
 * counts: an open that a later open overtook, or a refresh that a later
 * refresh overtook, is dropped when it lands (rule 4; the backend cancels
 * the older status read, which then answers `cancelled`).
 */
export function createRepoStore(backend: RepoBackend = tauriBackend): RepoStore {
  const [state, setState] = createStore<RepoFields>({
    recent: [],
    opening: false,
    status: null,
    error: null,
    watchWarning: null,
  });
  let openSeq = 0;
  let refreshSeq = 0;

  async function loadRecent(): Promise<void> {
    const outcome = await settle(() => backend.listRecentRepos());
    if (outcome.status === 'ok') {
      setState('recent', outcome.data);
    }
  }

  async function open(path: string): Promise<void> {
    const seq = ++openSeq;
    setState({ opening: true, error: null });
    const outcome = await settle(() => backend.openRepo(path));
    if (seq !== openSeq) {
      return;
    }
    setState('opening', false);
    if (outcome.status === 'error') {
      setState('error', outcome.error);
      return;
    }
    // Refreshes of the previous repository are now stale.
    refreshSeq++;
    setState({ status: outcome.data.status, watchWarning: outcome.data.watch_error });
    await loadRecent();
  }

  async function openFromDialog(): Promise<void> {
    let path: string | null;
    try {
      path = await backend.pickFolder();
    } catch (error) {
      setState('error', bridgeError(error));
      return;
    }
    if (path !== null) {
      await open(path);
    }
  }

  async function refresh(): Promise<void> {
    const id = state.status?.repo.id;
    if (id === undefined) {
      return;
    }
    const seq = ++refreshSeq;
    const outcome = await settle(() => backend.getStatus(id));
    if (seq !== refreshSeq || state.status?.repo.id !== id) {
      return;
    }
    if (outcome.status === 'ok') {
      setState('status', outcome.data);
    } else if (outcome.error.kind !== 'cancelled') {
      setState('error', outcome.error);
    }
  }

  // Event subscriptions end with the owner. `listen` resolves
  // asynchronously, so a subscription that lands after disposal is undone at
  // once; one that fails (no Tauri host) leaves nothing to undo.
  let disposed = false;
  const unlisteners: Unlisten[] = [];
  function subscribe(listen: () => Promise<Unlisten>): void {
    let pending: Promise<Unlisten>;
    try {
      pending = listen();
    } catch {
      return;
    }
    pending.then(
      (unlisten) => {
        if (disposed) {
          unlisten();
        } else {
          unlisteners.push(unlisten);
        }
      },
      () => {},
    );
  }
  subscribe(() =>
    backend.onRepoChanged((event) => {
      if (state.status?.repo.id === event.repo_id) {
        void refresh();
      }
    }),
  );
  subscribe(() =>
    backend.onWatchFailed((event) => {
      if (state.status?.repo.id === event.repo_id) {
        setState('watchWarning', event.error);
      }
    }),
  );
  onCleanup(() => {
    disposed = true;
    for (const unlisten of unlisteners.splice(0)) {
      unlisten();
    }
  });

  void loadRecent();

  return {
    state,
    open,
    openFromDialog,
    refresh,
    dismissError: () => setState('error', null),
  };
}
