import type { FileStatus, RepoInfo, StatusEntry } from '@shootgit/ipc-types';

/**
 * Plain-text renderings of the status models for the raw Phase 0 list
 * (P0-12). They mirror what `git status --porcelain` would print, so the
 * list can be checked against a terminal at a glance.
 *
 * Not localised: these are git's own codes and ref names. The words in
 * `describeHead` go through `t()` once it lands (P6-20).
 */

/** Git's letter for each half of the `XY` code. */
const LETTER: Record<FileStatus, string> = {
  unmodified: '.',
  modified: 'M',
  type_changed: 'T',
  added: 'A',
  deleted: 'D',
  renamed: 'R',
  copied: 'C',
  unmerged: 'U',
  untracked: '?',
  ignored: '!',
};

/** The entry's two-letter `XY` code: `.M`, `R.`, `UU`, `??`, `!!`. */
export function statusCode(entry: StatusEntry): string {
  return LETTER[entry.index_status] + LETTER[entry.worktree_status];
}

/** Commit ids are shown with their first seven characters, as git does. */
function short(oid: string): string {
  return oid.slice(0, 7);
}

/** Where HEAD points: `main @ 0123456`, `main (no commits yet)`, `detached at 0123456`. */
export function describeHead(info: RepoInfo): string {
  const head = info.head;
  switch (head.kind) {
    case 'branch':
      return `${head.name} @ ${short(head.oid)}`;
    case 'unborn':
      return `${head.name} (no commits yet)`;
    case 'detached':
      return `detached at ${short(head.oid)}`;
  }
}

/**
 * The upstream and how far HEAD is from it: `origin/main ↑1 ↓2`. Empty
 * without an upstream; `(gone)` when the upstream branch no longer exists.
 */
export function describeUpstream(info: RepoInfo): string {
  if (info.upstream === null) {
    return '';
  }
  if (info.ahead_behind === null) {
    return `${info.upstream} (gone)`;
  }
  return `${info.upstream} ↑${info.ahead_behind.ahead} ↓${info.ahead_behind.behind}`;
}
