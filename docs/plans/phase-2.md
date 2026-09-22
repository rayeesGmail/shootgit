# Phase 2 — Log & branches (4 weeks, ends 2026-11-29)

Spec: §3 G5–G7, G9 (stash); §5 Log, Graph layout, Branch ops, Cherry-pick/revert/reset, Stash, Undo; §6 Log view, Branch popup; Appendix A6, A7.
Goal: a 60 fps commit graph on 500k commits with IntelliJ's commit and branch actions.

## Engine

- [ ] **P2-01** Ref listing via `gix`: local branches, remote branches, tags, HEAD, upstream + ahead/behind per branch; CLI fallback `for-each-ref --format=... -z`. Tests on fixture with 200 branches and annotated + lightweight tags. · model: opus
- [ ] **P2-02** Log walk via `gix` revwalk (topo order, all refs or a filter set), paged 500 rows, `Commit` model incl. signature status via `git verify-commit` lazily. CLI fallback `git log --format=%H%x00%P%x00%an%x00...%x00 -z`. Tests: page boundaries stable; merge parents ordered. · model: opus
- [ ] **P2-03** Graph lane assignment in Rust: `GraphRow { lane, parent_lanes, colour }` with colour reuse, streamed per page. Golden tests on hand-built DAGs (linear, one merge, octopus, criss-cross). · model: best
- [ ] **P2-04** Log filters: branch include/exclude, author, date range, path (with `--follow` for single file), text/regex/match-case; composed into revwalk or CLI args. Tests per filter and combination. · model: opus
- [ ] **P2-05** Commit details: message, author/committer/dates, parents, refs, changed files with per-file diff (`git diff-tree -p -z`), merge commit diff vs parent 1 or 2. Tests on merge fixture. · model: opus
- [ ] **P2-06** Branch ops: create (from ref, checkout, overwrite), switch (smart: autostash/shelve dirty tree, force option), rename, delete local (`-d`/`-D` with confirmation flag), delete remote (`push --delete`), set upstream, compare (`rev-list --left-right --count`, commit lists both ways, file diff between tips). Tests incl. delete-with-unmerged refusal. · model: opus
- [ ] **P2-07** Restore deleted branch via reflog: record sha before delete; `undo_branch_delete()`. Test. · model: opus
- [ ] **P2-08** Cherry-pick (single/multiple, `-x` suffix option, no-commit option), revert (auto-commit option), reset (soft/mixed/hard/keep; hard snapshots dirty files to safety shelf), checkout revision (detached), create tag (lightweight/annotated), delete tag, push tags. Continue/abort/skip for in-progress cherry-pick/revert. Tests for each incl. conflict stop state. · model: opus
- [ ] **P2-09** Stash: push (message, keep-index, paths), list `-z`, show, pop, apply, drop, branch-from-stash. Tests. · model: opus
- [ ] **P2-10** SafetyPoint + Undo v1: record before reset/cherry-pick/revert/branch delete; `undo_last()` maps op type to `reset --keep` / `rebase --abort` / branch recreate; list last 10 with labels. Tests: reset then undo restores HEAD and index. · model: opus
- [ ] **P2-11** Repo state detection: `RepoState` (clean, merging, rebasing, cherry-picking, reverting, bisecting) from `.git` markers; exposed in `RepoInfo`; updated by watcher. Tests with fixtures for each state. · model: opus
- [ ] **P2-12** Perf: log first page < 200 ms on 500k-commit synthetic fixture; page fetch < 50 ms; bench added. · model: opus
- [ ] **P2-24** Bounded Log memory: fixed LRU window of 20k rows, pages re-fetched from `gix` on scroll, cancelled when the viewport moves on; Log walk starts only when the view is visible and stops when hidden; inactive repos have no walker. Test: 500k-commit fixture keeps RSS growth < 60 MB while scrolling end to end on the constrained VM. · model: opus

## UI

- [ ] **P2-13** Log view: virtualized rows (custom, not a DOM list), `<canvas>` graph column drawn per visible range from `GraphRow`s, ref labels (compact toggle), subject, author, date columns; prefetch 3 screens ahead. · model: opus
- [ ] **P2-14** Details pane: message, metadata, parents as links, changed-file list with diff viewer reuse from Phase 1; merge parent toggle. · model: sonnet
- [ ] **P2-15** Filter bar: branch picker (include/exclude via right-click), author combo, date presets + range, path chooser, text with regex/match-case; highlight my commits / merge commits; Go to hash/branch/tag. · model: sonnet
- [ ] **P2-16** Commit context menu: checkout revision, new branch/tag from here, cherry-pick, revert, reset (submenu), undo commit (if HEAD), compare with local, compare selected commits, copy hash/message, create patch, open in browser (placeholder until Phase 4). · model: sonnet
- [ ] **P2-17** Branch popup (⌘B): current with ahead/behind, favourites, recent, local, remote, tags; search; Enter = checkout, ⌘Enter = checkout-and-rebase; context menu: new branch from, compare with current, show diff with working tree, rebase current onto, merge into current, update, push, rename, delete, edit upstream, favourite. · model: sonnet
- [ ] **P2-18** Branches side panel in Log with grouping by prefix (`feature/…`) and the same actions. · model: sonnet
- [ ] **P2-19** Stash UI: list with preview diff, pop/apply/drop/branch; "Stash changes" action with message and keep-index. · model: sonnet
- [ ] **P2-20** Undo: bottom-bar Undo button + ⌘Z (outside text fields) showing label of last safety point; notification toasts with Undo for reset, cherry-pick, branch delete. · model: sonnet
- [ ] **P2-21** Detached HEAD banner with "Create branch here"; repo state banner for in-progress cherry-pick/revert with Continue/Abort/Skip. · model: sonnet
- [ ] **P2-22** Multi-repo: sidebar list, per-repo Log root column and root filter; repo switcher in toolbar. · model: sonnet
- [ ] **P2-23** Marketing: post the log-graph demo; newsletter #2. · model: sonnet

## Exit criteria

- Log scrolls at 60 fps on the 500k-commit fixture on all 3 OSes (measured with the built-in frame counter).
- Every action in Appendix A6 and A7 marked 1.0 is available from context menu, palette and keyboard.
- Undo restores state after reset, cherry-pick and branch delete (tests).
- G5, G6, G7, G9 (stash part), G13 (partial) verified.
