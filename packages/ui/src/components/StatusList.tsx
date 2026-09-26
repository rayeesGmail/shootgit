import type { StatusEntry } from '@shootgit/ipc-types';
import { For, type JSX, Show } from 'solid-js';

import { statusCode } from '../lib/status';

/**
 * At most this many rows are rendered (SPEC §4 Low-resource operation,
 * rule 8: no unbounded DOM above 500 files). The virtualised Changes tree
 * replaces this raw list in Phase 1.
 */
export const MAX_ROWS = 500;

export interface StatusListProps {
  entries: readonly StatusEntry[];
}

/**
 * The raw status of the open repository: one row per path that is not clean,
 * in git's order, as `XY path` like `git status --porcelain` (P0-12).
 *
 * Strings are hardcoded English until `t()` lands (P6-20).
 */
export function StatusList(props: StatusListProps): JSX.Element {
  const shown = (): readonly StatusEntry[] => props.entries.slice(0, MAX_ROWS);
  const hidden = (): number => props.entries.length - shown().length;

  return (
    <Show
      when={props.entries.length > 0}
      fallback={<p class="status-list__clean">Nothing to commit, working tree clean.</p>}
    >
      <ol class="status-list">
        <For each={shown()}>
          {(entry) => (
            <li class="status-list__row">
              <span class="status-list__code">{statusCode(entry)}</span>
              <span class="status-list__path">{entry.path}</span>
              <Show when={entry.old_path}>
                {(oldPath) => <span class="status-list__old">← {oldPath()}</span>}
              </Show>
              <Show when={entry.is_submodule}>
                <span class="status-list__flag">submodule</span>
              </Show>
            </li>
          )}
        </For>
      </ol>
      <Show when={hidden() > 0}>
        <p class="status-list__more">…and {hidden()} more</p>
      </Show>
    </Show>
  );
}
