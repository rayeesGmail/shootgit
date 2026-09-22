# Phase 3 — Merge & interactive rebase (4 weeks, ends 2026-12-27)

Spec: §3 G8, G9 (shelve), G10, G13; §5 Interactive rebase, Merge conflicts, Shelve, Undo; §6 Merge tool, Interactive rebase; Appendix A2 (shelve/patch), A9, A10.
Goal: resolve a 20-file conflicting rebase end to end without a terminal.

## Engine

- [ ] **P3-01** Conflict model: list unmerged paths from status; read stages `:1:` `:2:` `:3:` via `git show` (or `gix` blob reads); detect binary, delete/modify, add/add, rename conflicts. Tests on a fixture with each conflict type. · model: opus
- [ ] **P3-02** Three-way chunking: diff3-style algorithm producing `ConflictChunk { base, ours, theirs, state: conflict|ours_only|theirs_only|same }`; auto-resolve non-conflicting chunks; "resolve simple conflicts" heuristic (whitespace-only, identical changes). Golden tests. · model: best
- [ ] **P3-03** Resolution write: write result bytes (preserve EOL), `git add` to mark resolved, `git checkout --ours/--theirs` for whole-file accept; `mark_unresolved` via `git checkout -m`. Tests. · model: opus
- [ ] **P3-04** Merge: `merge(branch, opts: no_ff, squash, no_commit, message)`; continue/abort; conflict stop detection. Tests incl. squash merge and abort restoring tree. · model: opus
- [ ] **P3-05** Sequence-editor mode of the app binary: `<app> --seq-editor <todo-file>` writes the prepared todo; `<app> --msg-editor <file>` writes prepared message; both exit 0 immediately. Tests: binary invoked by `git rebase -i` with `GIT_SEQUENCE_EDITOR` on a fixture produces expected history. · model: best
- [ ] **P3-06** Interactive rebase: `plan(base) -> RebasePlan` (commits, default pick), `start(plan)` with reorder/squash/fixup/reword/drop/edit, stop detection (`edit`, conflict), `continue/skip/abort`, reword message capture for squash. Tests: reorder, squash 3→1, drop, reword, edit stop and continue. · model: best
- [ ] **P3-07** Reword any commit and squash/fixup selected commits from Log without opening the planner (generate plan automatically). Tests. · model: opus
- [ ] **P3-08** Rebase onto branch (`git rebase <onto>`), checkout-and-rebase, rebase with `--autostash` and `--update-refs` option; conflict stop integrates with P3-01. Tests. · model: opus
- [ ] **P3-09** Shelve: create (name, all/selected files/selected hunks) saving patch to `.git/<app>/shelves/<id>.patch` + metadata, then revert those changes; list, preview, rename, delete, restore deleted (kept 30 days); unshelve all/selected with `git apply --3way`, optional delete-after; silently shelve; import patch into shelf. Tests incl. binary files and untracked files in shelf. · model: opus
- [ ] **P3-10** Patches: create patch from changes or commit range to file/clipboard; apply patch from file/clipboard with 3-way and into-shelf option. Tests. · model: opus
- [ ] **P3-11** Smart checkout / move changes to another branch: shelve → switch → unshelve, with conflict handling. Tests. · model: opus
- [ ] **P3-12** Undo v2: rebase/merge undo via `ORIG_HEAD`/reflog; stash-pop and unshelve conflicts abortable; safety points labelled. Tests. · model: opus

## UI

- [ ] **P3-13** Conflicts banner + Conflicts panel: file list with Accept Yours / Accept Theirs / Merge / Show diff / Compare with branch; progress count. · model: sonnet
- [ ] **P3-14** Three-pane merge tool (CodeMirror): ours · result · theirs, colour-coded chunks, ›‹ apply arrows per chunk, Accept left/right/both, "Apply all non-conflicting", "Resolve simple conflicts", editable result, ignore-whitespace, sync scroll, Apply/Abort, keyboard navigation between conflicts. · model: opus
- [ ] **P3-15** Repo-state banner for merge/rebase/cherry-pick in progress with Continue / Skip / Abort and "Open conflicts". · model: sonnet
- [ ] **P3-16** Interactive rebase planner: table of commits (action dropdown, inline reword, drag/reorder, ⌘↑/↓), preview of resulting history, Start; stop-for-edit banner with Continue. · model: opus
- [ ] **P3-17** Log actions wired: Rebase interactively from here, Edit message, Squash, Fixup, Drop; Merge branch dialog (no-ff, squash, no-commit, message); Rebase dialog (onto, interactive, autostash). · model: sonnet
- [ ] **P3-18** Shelves view: list, preview diff, unshelve (all/selected, delete after), rename, delete, restore deleted; "Shelve changes" action in Changes view (name, selection); silently shelve shortcut (⌘⇧S). · model: sonnet
- [ ] **P3-19** Patch actions: Create patch (dialog: file or clipboard), Apply patch (file/clipboard, 3-way, into shelf). · model: sonnet
- [ ] **P3-20** Move changes to another branch action in the branch popup and Changes view. · model: sonnet
- [ ] **P3-21** Marketing: "interactive rebase without fear" demo video; newsletter #3. · model: sonnet

## Exit criteria

- A scripted 20-file conflicting rebase (`scripts/e2e/conflict-rebase.sh`) is resolved fully in the UI on all 3 OSes.
- Interactive rebase reorder/squash/fixup/reword/drop/edit verified by tests.
- Shelve round-trips binary, CRLF and untracked files byte-identically.
- Undo works for merge and rebase.
- G8, G9, G10, G13 verified; Appendix A9 and A10 complete.
