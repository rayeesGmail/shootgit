# Phase 0 — Foundation (2 weeks, ends 2026-10-04)

Spec: §4 Architecture, §5 Git binary resolution + Process model, §9 Platform rules.
Goal: an app that opens a repo and shows raw `git status` output on all 3 OSes, with CI green.

## Tasks

- [x] **P0-01** Cargo workspace with empty lib crates: `git-engine`, `git-engine-cli`, `forge-core`, `forge-github`, `forge-gitlab`, `credential-helper`; shared `[workspace.dependencies]`; `rust-toolchain.toml`; `cargo test --workspace` passes with one trivial test per crate. · model: opus
- [x] **P0-02** GitHub Actions `ci.yml`: matrix (ubuntu, macos, windows) running fmt, clippy `-D warnings`, `cargo test --workspace`. Cache cargo. Required check on PRs. · model: opus
- [x] **P0-03** Tauri 2 app in `src-tauri` + `packages/ui` (Solid + Vite + TypeScript strict). `pnpm tauri dev` opens a window titled with the app name on all 3 OSes. pnpm workspace at repo root. · model: opus
- [x] **P0-04** `specta` + `tauri-specta` wired; `pnpm gen:types` writes `packages/ipc-types/bindings.ts` (committed, never hand-edited); one sample command `ping() -> String` round-trips from UI; CI job runs `pnpm gen:types && git diff --exit-code packages/ipc-types/bindings.ts` and fails if the committed file is stale. · model: opus
- [x] **P0-05** `git_engine::git_binary`: resolver order (settings path → PATH git ≥ 2.30 → bundled placeholder), macOS Xcode-stub detection (`xcode-select -p` fails ⇒ skip `/usr/bin/git`), version parsing. Unit tests with fake PATH dirs per OS. · model: opus
- [x] **P0-06** `git_engine::process::GitCommand`: builder over `tokio::process`, always passes `--no-optional-locks -c core.quotepath=off`, sets `CREATE_NO_WINDOW` on Windows, `GIT_TERMINAL_PROMPT=0`, timeout, captures stdout/stderr as bytes, `-z` split helper. Tests: timeout fires; NUL split handles empty fields; exit code mapping to `GitError`. Reserve env-injection points for `GIT_ASKPASS`/`SSH_ASKPASS` and a per-spawn `-c` list (used by P1-25 and P4-13). Build it on a generic process builder in the same module (`CREATE_NO_WINDOW`, login-shell PATH, timeout, byte capture) that non-git tools reuse (`ssh`, `ssh-add`, `ssh-keygen` in P1-25, P1-26, P4-27). · model: opus
- [x] **P0-17** Low-resource runtime and limiter (§4 Low-resource operation): build one tokio runtime sized `max(1, available_parallelism() - 1)` worker threads and hand it to Tauri via `tauri::async_runtime::set(runtime.handle().clone())` before `tauri::Builder::default()` so the app has a single runtime; `GitCommand` goes through a shared concurrency limiter (semaphore of 2 when available_parallelism() ≤ 4, else 4) with two priority lanes (visible-view work before background work); every read accepts a `CancellationToken` and stops promptly when cancelled; `perf/` harness (`crates/perf-harness` or `scripts/perf/`) records RSS, git spawn count and wall time per named operation to JSON. Tests: limiter never exceeds its cap under 50 concurrent requests; high-priority request completes before queued low-priority ones; cancelled read returns within 50 ms. · model: best
- [x] **P0-07** macOS login-shell environment: resolve `PATH` once via the user's shell (`$SHELL -ilc 'echo $PATH'`) with a 2 s timeout and fallback; used by `GitCommand`. Test on macOS runner. · model: opus
- [x] **P0-08** `git_engine::status`: run `git status --porcelain=v2 -z --branch --untracked-files=all` and parse into `RepoInfo` + `Vec<StatusEntry>` (ordinary, renamed/copied, unmerged, untracked, ignored). Fixture script `scripts/fixtures/basic.sh` creates a repo with each entry kind. Tests for all kinds incl. rename with spaces and unicode path. · model: opus
- [x] **P0-09** `RepoActor`: tokio task per repo owning `Repo` handle, mailbox for commands, serialises writes, concurrent reads. `open_repo(path)` discovers `.git` (incl. worktree `.git` file). Test: two concurrent reads, one write, ordering preserved. · model: opus
- [x] **P0-10** File watcher: `notify` recursive watch on worktree + `.git` (HEAD, index, refs/, packed-refs, *_HEAD, rebase-merge/, rebase-apply/, logs/), respects `.gitignore` via `ignore` crate, coalesces into `RepoChanged { kinds }` after 150 ms (250 ms Windows). Own-write suppression via generation counter. Tests: touch file → one event; edit `.git/HEAD` → `kinds` contains `head`. · model: best
- [ ] **P0-11** Settings store: `settings.json` in Tauri `app_config_dir`, typed `Settings` struct, recent repos list (max 20), git binary override, atomic write. Tests: round-trip, corrupt file recovers to defaults. · model: opus
- [ ] **P0-12** Tauri commands `open_repo`, `list_recent_repos`, `get_status`; events `repo-changed`. UI: minimal window with "Open repository" (native dialog) and a raw list of status entries that refreshes on `repo-changed`. Typed events need three things P0-04 left out on purpose: tauri-specta's `derive` feature (for the `Event` macro), `.events(collect_events![..])` on `ipc::builder`, and `builder.mount_events(app)` in `run()`'s `setup` — without the last one the frontend never receives events. Replaces the `ping` status line in `App.tsx` with real repository state. · model: opus
- [ ] **P0-13** `git-engine-cli`: `status <path>` and `watch <path>` subcommands printing JSON, for manual testing. · model: opus
- [ ] **P0-18** Constrained-VM perf gate: `scripts/fixtures/large-synthetic.sh` generates a repo with 100k files and 200k commits via `git fast-import` in under 2 min and caches it in CI; `scripts/perf/smoke.sh` uses `git-engine-cli` to open the repo, run status, watch for one external touch, and shut down, emitting the harness JSON; CI job `perf-constrained` runs it inside a 4 GB / 2 vCPU cgroup (Docker `--memory 4g --cpus 2` on ubuntu runner), compares to `perf/baseline.json`; peak RSS and git spawn count fail on > 10 % regression; wall time is the median of 3 runs and fails on > 25 %; when no baseline exists the job passes, uploads the generated `baseline.json` as a workflow artifact and emits a warning annotation. Definition of done: run the job once via `workflow_dispatch`, download the artifact, commit it as `perf/baseline.json` with a `perf: seed baseline` commit, and re-run to confirm the gate compares rather than seeds. Baseline changes only via explicit `perf: update baseline` commits. · model: opus
- [ ] **P0-14** Release workflow skeleton `release.yml` (tag `v*`, build matrix for 5 targets, artifacts uploaded, signing steps present but skipped when secrets are absent). Dry run on a `v0.0.1-test` tag. · model: opus
- [ ] **P0-15** Repo hygiene: `.editorconfig`, `rustfmt.toml`, `.prettierrc`, `LICENSE-THIRD-PARTY` generation via `cargo about` + `license-checker` (stub), `CONTRIBUTING.md` pointing at CLAUDE.md. Exclude `packages/ipc-types/bindings.ts` from both Prettier (`.prettierignore`) and `.editorconfig`'s `trim_trailing_whitespace`: specta writes tabs and a trailing blank line, so any reformat makes the committed file differ from a fresh export and breaks the P0-04 freshness check (ADR 0003). · model: sonnet
- [ ] **P0-16** Landing-page waitlist stub: `site/` folder with a static page and a newsletter form (provider TBD), deployed to GitHub Pages. Marketing starts now (§10). · model: sonnet

## Exit criteria

- App opens a repo and renders live `git status` on macOS, Windows and Linux.
- Watcher reflects an external `touch` within 300 ms on all 3 OSes.
- CI matrix green; `pnpm gen:types` idempotent; release dry-run produces artifacts.
- No `unwrap()` outside tests; clippy clean.

## Follow-ups

Gaps found while reviewing finished tasks. Each one names the task that should
close it. When that task starts, move the item into its scope and delete it here.

- From P0-02: `.gitattributes` has only narrow rules: `bindings.ts`,
  `crates/*/tests/golden/**` (P0-17) and `scripts/fixtures/**` (P0-08).
  Windows runners check files out as CRLF, which will break the byte-exact
  fixture and golden patch tests. Add the general `-text` or `eol` rules for
  golden files and fixture inputs. Owner: P0-15, and it must land before
  P1-01.
- From P0-02: Dependabot, `cargo audit` and `npm audit` (spec "Dependency
  audit") have no plan task. Add one.
- From P0-02: the workflows are not linted. Run `actionlint` on `ci.yml`
  and on the new `release.yml`. Owner: P0-14.
- From P0-03: CI caches cargo but not the pnpm store. Owner: P0-18 (CI cache
  work), or any later CI change.
- From P0-03: the shell's English UI strings bypass `t()`. Owner: P6-20.
- From P0-03: `README.md` has no build instructions. Owner: P0-15.
- From P0-03: the CSP keeps `style-src 'unsafe-inline'`
  (`src-tauri/tauri.conf.json`) for CodeMirror. Once the diff editor lands in
  Phase 1, check whether it can be dropped.
- From P0-04: ADR 0005 (exact pin of the specta rc crates) has no row in the
  spec's §12 Decisions log. Add it in the Claude Doc and re-export; never
  hand-edit `SPEC.md`.
- From P0-05: an unusable git path in settings (missing, not executable, too
  old, or the Xcode stub) is a hard error, not a fallback to PATH. §5 does
  not say which is right. Confirm it and record it with `/adr`.
- From P0-05: the 2.30 minimum is enforced on the settings and bundled
  sources too, though §5 attaches it only to PATH (CLAUDE.md makes 2.30 the
  overall minimum). Confirm it in the same ADR.
- From P0-05: `bundled_git_path()` is always `None`. Shipping a bundled git
  (§9) has no plan task yet. Add one to the packaging work.
- From P0-06: `ProcessCommand` cannot write to the child's stdin. Needed to
  feed patches to `git apply` and for the credential protocol. Owner: P1-04.
- From P0-06: all of stdout is held in memory. Large diffs and logs need a
  streaming variant (§4 bounded memory). Owner: P1-02 (diffs); P2-02 uses
  `gix` for the Log.
- From P0-06: no locale is forced, so git's stderr comes out in the user's
  language. Set `LC_ALL=C` (or equivalent) for spawns whose stderr is
  classified. Owner: P1-10.
- From P0-06: `DEFAULT_TIMEOUT` is 60 s and the spec gives no value. Confirm
  or change it, and record the choice in the spec.
- From P0-06: on Windows, a process that git starts between spawn and
  `AssignProcessToJobObject` escapes the Job Object and survives a tree kill
  (documented in `process/tree.rs`). Revisit if a leaked child ever shows up,
  e.g. by spawning suspended. P0-17 left it open: the fix is Windows-only and
  could not be checked on macOS. Owner: unassigned; give it to a task whose
  work is verified on Windows.
- From P0-06: children run in their own process group on Unix, so one that
  reads `/dev/tty` (ssh asking for a passphrase) is stopped by SIGTTIN instead
  of prompting in `git-engine-cli`. The GUI is unaffected. Owner: P1-25 (askpass
  replaces terminal prompts); until then the CLI cannot answer prompts.
- From P0-17: `perf-harness` records this process's peak RSS, plus the
  largest single child's peak on Unix (`None` on Windows). The spec's "peak
  RSS, all processes" may need the sum across git children. Owner: P0-18.
- From P0-17: the runtime caps worker threads at `max(1, cores - 1)` but
  leaves tokio's blocking pool at its default (up to 512 threads on demand).
  Nothing uses it yet. Decide whether the spec's thread budget covers it and
  record the answer with `/adr`.
- From P0-07: ADR 0006 (login-shell `PATH` via `git_engine::process`
  instead of `fix-path-env`) has no row in the spec's §12 Decisions log, and
  §9's table still names `fix-path-env`. Update the Claude Doc and
  re-export; never hand-edit `SPEC.md`.
- From P0-07: only `PATH` is taken from the login shell, not `SSH_AUTH_SOCK`,
  `LANG` or the rest of the environment. Tools spawned before
  `login_shell::init()` finishes get this process's `PATH`; git resolution
  avoids that by awaiting `ResolveOptions::from_login_shell_env`. The `ssh`,
  `ssh-add` and `ssh-keygen` spawns must await `init()` too and extend
  `login_shell` for any other variables they need (ADR 0006). Owner: P1-25.
- From P0-07: the probe timeouts (10 s for `git --version`, 5 s for
  `xcode-select -p`) are not in the spec. Confirm or change them, and record
  the choice in the spec with the `DEFAULT_TIMEOUT` item above.
- From P0-07: some login shells fall back to this process's `PATH`: tcsh
  (`-ilc` fails), nushell, and rc files that `exec tmux` (they hit the 2 s
  timeout; a tmux server started that way survives the kill). An unset or
  relative `$SHELL` also falls back; the shell is not read from `getpwuid`.
  Revisit if users report git not being found. Owner: unassigned.
- From P0-08: the status models (`Status`, `RepoInfo`, `Head`,
  `StatusEntry`, `FileStatus`) have no serde or specta derives. P0-12 needs
  both and P0-13 needs `Serialize`. Paths are `PathBuf` holding git's raw
  bytes on Unix, and serde fails on a path that is not UTF-8; decide how such
  paths cross IPC. Owner: P0-12.
- From P0-08: `RepoInfo` lacks the §5 fields `id` (Owner: P0-12, which owns
  the open-repo registry and IPC ids) and `state` (Owner: P2-11, from the
  markers in `.git`).
- From P0-08: `status` always runs in the visible lane of the git limiter.
  A repo that is not active must refresh in the background lane (§4
  Low-resource operation). Add a priority option when that first happens.
  Owner: unassigned.
- From P0-08: status output is held in memory and parsed on an async worker
  in one pass. Check it against the < 300 ms warm budget on the 50k-file
  fixture. Owner: P1-12.
- From P0-08: the `fixture()` helper lives in `tests/status.rs`. Move it to a
  shared test-support module when a second test file builds fixtures.
  Owner: P1-01.
- From P0-08: the fixture test runs `bash` from `PATH`. CI uses Git Bash on
  Windows, but a local run from PowerShell without Git's `usr\bin` on `PATH`
  may get WSL's `bash.exe` or none. Document it in `CONTRIBUTING.md`.
  Owner: P0-15.
- From P0-08: two P0-05 tests with 300 ms probe timeouts
  (`hanging_version_probe_times_out_and_the_candidate_is_skipped`,
  `xcode_select_probe_reads_the_exit_status_and_times_out`) failed once under
  a loaded full-workspace run on macOS and passed on rerun. P0-10's review saw
  them fail again in an unloaded serial `cargo test --workspace` (the lib
  binary now also runs 23 watcher unit tests), then pass three times in a
  row. Loosen their timing before they flake in CI. Owner: unassigned.
- From P0-08: `RUSTDOCFLAGS="-D warnings" cargo doc -p git-engine --no-deps`
  fails on two redundant explicit link targets (`git_binary.rs:159`, `:177`).
  CI does not run rustdoc. Fix them and add a doc check. Owner: P0-15, or any
  later CI change.
- From P0-09: ADR 0007 (clear inherited repository-local git variables on
  every spawn) has no row in the spec's §12 Decisions log. Add it in the
  Claude Doc and re-export; never hand-edit `SPEC.md`.
- From P0-09: `open_repo` does not honour `GIT_CEILING_DIRECTORIES`, git's
  stop at file-system boundaries, `core.worktree` (bare dotfiles repos) or
  `safe.directory`. A repository owned by another user opens, and every git
  call then fails with "dubious ownership". Surface that error with the fix
  (`git config --global --add safe.directory`, which the app must not run
  itself). Owner: P0-12; the rest unassigned.
- From P0-09: `open_repo` is synchronous and walks the tree with `stat` calls
  and small reads, so it can block an async worker on a network drive.
  Decide in P0-12 whether to call it through `spawn_blocking` (see the P0-17
  blocking-pool item). Owner: P0-12.
- From P0-09: a write holds back every read submitted after it. Once
  `fetch`/`pull`/`push` exist, a long network write would freeze status and
  diffs. Decide whether network phases run outside the actor, with only the
  ref update serialised. Owner: P1-09.
- From P0-10: §4 says the `RepoActor` "owns the watcher", but
  `git_engine::watcher::Watcher` is a standalone handle beside the actor.
  Compose them per repository (the actor holds the `Watcher`, and every
  engine write takes `begin_write()` and keeps the guard through the
  post-write status snapshot, §5 rule 2), or record the split with `/adr`.
  `RepoChanged` also needs serde and specta derives. Owner: P0-12.
- From P0-10: no polling fallback (§5 rule 8) and no handling of a repo
  moved or deleted (§5 rule 9). A failed watcher ends its stream with
  `WatchError::Stopped`; `is_watch_limit()` names the inotify case. Nothing
  restarts or polls yet. Owner: P0-12 for surfacing the error; the polling
  fallback needs a plan task.
- From P0-10: on Linux, notify's recursive mode registers inotify watches
  inside ignored directories too, so a large `node_modules/` or `target/` can
  exhaust `max_user_watches`. A walker-driven per-directory watch that skips
  ignored dirs would fix it. Owner: unassigned (Linux-verified task).
- From P0-10: `RepoChanged` carries kinds only. §5 row 1 "re-run status for
  changed paths only" needs a bounded list of changed paths. Owner: P1-12.
- From P0-10: files that are tracked but match `.gitignore` (force-added)
  are dropped by the ignore filter, so their working-tree edits produce no
  event; only index changes are reported. Owner: unassigned.
- From P0-10: `DEFAULT_LOCK_HOLD` (2 s) and the event channel capacity (16)
  are not in the spec. Confirm them with the `DEFAULT_TIMEOUT` item above.
  Global excludes are read at start and on rule reload, not watched.
- From P0-10: the watcher starts its OS watch on tokio's blocking pool
  (`spawn_blocking`), the first user of it. Relevant to the P0-17
  blocking-pool item.
- From P0-10: notify's Windows backend uses a 16 KB ReadDirectoryChangesW
  buffer and may not signal a rescan on overflow, so a large burst could be
  lost. Late FSEvents deliveries after an `OwnWrite` guard drops cost one
  extra refresh. Measure both in the P0-18 smoke. Owner: P0-18.
