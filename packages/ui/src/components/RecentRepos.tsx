import { For, type JSX, Show } from 'solid-js';

export interface RecentReposProps {
  paths: readonly string[];
  disabled: boolean;
  onOpen: (path: string) => void;
}

/**
 * The recently opened repositories, newest first; choosing one opens it.
 *
 * Strings are hardcoded English until `t()` lands (P6-20).
 */
export function RecentRepos(props: RecentReposProps): JSX.Element {
  return (
    <Show when={props.paths.length > 0}>
      <section class="recent">
        <h2 class="recent__title">Recent repositories</h2>
        <ul class="recent__list">
          <For each={props.paths}>
            {(path) => (
              <li>
                <button
                  type="button"
                  class="recent__item"
                  disabled={props.disabled}
                  onClick={() => props.onOpen(path)}
                >
                  {path}
                </button>
              </li>
            )}
          </For>
        </ul>
      </section>
    </Show>
  );
}
