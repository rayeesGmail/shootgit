import { type JSX, Show } from 'solid-js';

import { APP_NAME } from './app-info';
import { Notice } from './components/Notice';
import { RecentRepos } from './components/RecentRepos';
import { StatusList } from './components/StatusList';
import { describeHead, describeUpstream } from './lib/status';
import { createRepoStore } from './stores/repo';

/**
 * The bare window for Phase 0 (P0-12): "Open repository" with the native
 * folder picker, and the raw status of the open repository, refreshed live
 * on `repo-changed`. The real shell (left rail, toolbar, status bar) is
 * P1-13 and the Changes view replaces the raw list in Phase 1.
 *
 * Strings are hardcoded English until the `t()` scaffolding lands (P6-20).
 */
export function App(): JSX.Element {
  const repo = createRepoStore();
  const state = repo.state;

  return (
    <main class="app">
      <header class="app__bar">
        <h1 class="app__title">{APP_NAME}</h1>
        <button
          type="button"
          class="app__action"
          disabled={state.opening}
          onClick={() => void repo.openFromDialog()}
        >
          Open repository…
        </button>
        <Show when={state.status}>
          <button type="button" class="app__action" onClick={() => void repo.refresh()}>
            Refresh
          </button>
        </Show>
      </header>

      <Show when={state.error}>
        {(error) => (
          <Notice
            tone="error"
            message={error().message}
            fix={error().fix}
            onDismiss={() => repo.dismissError()}
          />
        )}
      </Show>
      <Show when={state.status !== null ? state.watchWarning : null}>
        {(warning) => (
          <Notice
            tone="warning"
            message={`Changes made outside the app are not shown until you refresh: ${warning().message}`}
            fix={warning().fix}
          />
        )}
      </Show>

      <Show
        when={state.status}
        fallback={
          <section class="app__empty">
            <p class="app__hint">
              {state.opening ? 'Opening the repository…' : 'No repository is open yet.'}
            </p>
            <RecentRepos
              paths={state.recent}
              disabled={state.opening}
              onOpen={(path) => void repo.open(path)}
            />
          </section>
        }
      >
        {(status) => (
          <section class="repo">
            <p class="repo__path">{status().repo.path}</p>
            <p class="repo__head">
              {describeHead(status().repo)}
              <Show when={describeUpstream(status().repo)}>
                {(upstream) => <span class="repo__upstream">{upstream()}</span>}
              </Show>
            </p>
            <StatusList entries={status().entries} />
          </section>
        )}
      </Show>
    </main>
  );
}
