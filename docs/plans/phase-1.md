# Phase 1 — Changes & commit (4 weeks, ends 2026-11-01)

Spec: §3 G1–G4, G16; §5 Command mapping, Line-level staging algorithm, External change sync; §6 Changes view, Keyboard map.
Goal: daily-driveable for commit workflows, with hunk- and line-level staging that survives CRLF.

## Engine

- [ ] **P1-01** Diff model: `FileDiff`, `Hunk`, `DiffLine { kind, old_no, new_no, bytes, id }` with stable ids `(file, hunk_idx, line_idx)`. Parser for `git diff --patch -z --no-color -U3 --no-ext-diff` output incl. binary, mode change, rename header, `\ No newline at end of file`. Golden tests from fixtures (LF, CRLF, mixed, no-EOF newline, binary). · model: opus
- [ ] **P1-02** `diff_worktree(path)` and `diff_index(path)` commands; `diff_untracked(path)` synthesises an all-added diff. Test: each returns expected hunk counts on fixtures. · model: opus
- [ ] **P1-03** Patch builder: `build_patch(FileDiff, Selection) -> Vec<u8>` implementing §5 algorithm (unselected `+` removed, unselected `-` → context, headers recounted, bytes preserved). Golden tests: 8 cases incl. CRLF, partial hunk, only-deletions, whole-hunk, adjacent hunks, no-EOF newline. · model: best
- [ ] **P1-04** `stage_lines(selection)` = `git apply --cached --unidiff-zero --recount`; `unstage_lines` = same with `-R`. Integration tests verify index content via `git diff --cached` after apply on LF and CRLF fixtures; Windows runner included. · model: opus
- [ ] **P1-05** `stage_file`, `unstage_file`, `stage_all`, `unstage_all`, `add_untracked`, `remove_from_index` (keep worktree). Tests incl. renamed and deleted files. · model: opus
- [ ] **P1-06** Discard: `discard_lines`, `discard_file`, `discard_untracked` — each first writes a `SafetyPoint` shelf patch under `.git/<app>/safety/` then applies reverse patch / `git checkout --` / deletes. Test: discard then restore from safety patch yields identical bytes. · model: opus
- [ ] **P1-07** Commit: `commit(msg, amend, signoff, author_override)` via `git commit -F <tmp>`; reads `commit.template`; surfaces hook failures with stderr; detects CRLF warnings and large files (> 10 MB default) before commit and returns a structured warning. Tests with a failing pre-commit hook fixture. · model: opus
- [ ] **P1-08** Commit message history (last 50 per repo in `.git/<app>/messages.json`) and `last_commit_message()` for amend. · model: opus
- [ ] **P1-09** Remote ops: `fetch(remote, prune)`, `pull(mode: rebase|merge|ff_only, autostash)`, `push(remote, refspec, set_upstream, force_with_lease, tags)`. Streams progress lines as `op-progress`. Protected-branch regex check blocks force push. Tests against a local bare-repo fixture; force-with-lease rejection test. · model: opus
- [ ] **P1-10** Credential error detection: classify stderr (auth failed, host key unknown, SSH agent missing, 2FA) into `RemoteError` variants with user-facing guidance strings. · model: opus
- [ ] **P1-11** External change sync per §5 G16: index/HEAD/refs watchers trigger targeted refresh; own-write suppression validated; atomic-save (tmp + rename) coalesced. Test script `scripts/e2e/external-changes.sh` runs `git commit`, `git switch`, `git stash`, editor-style save and asserts events. · model: best
- [ ] **P1-12** Performance: status + diff of 50k-file fixture completes < 300 ms warm; benchmark in `benches/` with criterion; CI nightly records numbers. · model: opus
- [ ] **P1-24** Memory ceiling for Changes view: diffs over 5 MB or 50k lines load hunk-by-hunk; blob contents streamed; status entries above 500 files virtualised in the tree; CodeMirror instances pooled (max 3). Test: 5k changed files + 60k-line diff keeps RSS < 250 MB on the constrained-VM job; no main-thread stall > 100 ms measured via a frame-timing probe. · model: opus

## UI

- [ ] **P1-13** App shell: left rail (Changes, Log, PRs, Shelves, Settings placeholders), toolbar (repo switcher, branch name, Fetch, Push), bottom status bar with operation progress. Platform tokens (font, density) and light/dark. · model: sonnet
- [ ] **P1-14** `changes` store hydrated from `get_status`/`repo-changed`; file tree grouped Staged / Unstaged / Untracked with file-status colours; selection and keyboard navigation (J/K, arrows); Space toggles stage. · model: sonnet
- [ ] **P1-15** Diff view with CodeMirror 6 `@codemirror/merge`: side-by-side and unified toggle, word-level highlighting, collapse unchanged, ignore-whitespace toggle, next/prev change; windows hunks for 50k-line files. · model: opus
- [ ] **P1-16** Stage checkboxes: gutter checkbox per hunk and per line bound to `Selection`; Stage/Unstage selected buttons; optimistic update then reconcile on `repo-changed`; selection re-mapped by content after external edits (G16 rule 4). · model: opus
- [ ] **P1-17** Commit box: message editor with subject/body split and 72-col guide, template prefill, history dropdown, Amend toggle (loads last message), Sign-off toggle, author override field, Commit and Commit & Push buttons, pre-commit warnings banner (CRLF, large file, detached HEAD). · model: sonnet
- [ ] **P1-18** Discard, Add to .gitignore, Ignore (info/exclude), Copy path, Show in file manager in the file context menu; confirmations only for discard of untracked files. · model: sonnet
- [ ] **P1-19** Fetch/Pull/Push UI: toolbar buttons with progress, Push dialog (remote, target branch, set upstream, force-with-lease, tags), Pull mode from settings, error dialog with `RemoteError` guidance. · model: sonnet
- [ ] **P1-20** Command palette (⌘K/Ctrl+K) with fuzzy search over a registry of actions; keyboard map module with per-platform modifier; shortcuts from §6 table for implemented actions. · model: sonnet
- [ ] **P1-21** Onboarding when no repo is open: detect Git (or explain the fix), Open / Clone (URL + directory, progress) / Recent list. · model: sonnet
- [ ] **P1-22** Settings page v1: git binary path, pull mode, protected branches, confirmations, font and density. · model: sonnet
- [ ] **P1-23** Newsletter: first "building in public" email/post showing line-level staging GIF (marketing plan §10). · model: sonnet

## Exit criteria

- Author can stage individual lines, commit, and push a real repo from the app on all 3 OSes.
- Golden patch tests and CRLF integration tests green on Windows runner.
- External `git commit`/`switch`/editor save reflected in UI within 300 ms.
- Status refresh < 300 ms on 50k-file fixture (nightly bench).
- G1, G2, G3, G4, G16 verified by tests; keyboard map and palette reach every implemented action.
