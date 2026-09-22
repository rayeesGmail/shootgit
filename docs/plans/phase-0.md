# Phase 0 — Foundation (2 weeks, ends 2026-10-04)

Spec: §4 Architecture, §5 Git binary resolution + Process model, §9 Platform rules.
Goal: an app that opens a repo and shows raw `git status` output on all 3 OSes, with CI green.

## Tasks

- [ ] **P0-01** Cargo workspace with empty lib crates: `git-engine`, `git-engine-cli`, `forge-core`, `forge-github`, `forge-gitlab`, `credential-helper`; shared `[workspace.dependencies]`; `rust-toolchain.toml`; `cargo test --workspace` passes with one trivial test per crate. · model: opus
- [ ] **P0-02** GitHub Actions `ci.yml`: matrix (ubuntu, macos, windows) running fmt, clippy `-D warnings`, `cargo test --workspace`. Cache cargo. Required check on PRs. · model: opus
- [ ] **P0-03** Tauri 2 app in `src-tauri` + `packages/ui` (Solid + Vite + TypeScript strict). `pnpm tauri dev` opens a window titled with the app name on all 3 OSes. pnpm workspace at repo root. · model: opus
- [ ] **P0-04** `specta` + `tauri-specta` wired; `pnpm gen:types` writes `packages/ipc-types/bindings.ts`; one sample command `ping() -> String` round-trips from UI; CI fails if generated file is stale. · model: opus
- [ ] **P0-05** `git_engine::git_binary`: resolver order (settings path → PATH git ≥ 2.30 → bundled placeholder), macOS Xcode-stub detection (`xcode-select -p` fails ⇒ skip `/usr/bin/git`), version parsing. Unit tests with fake PATH dirs per OS. · model: opus
- [ ] **P0-06** `git_engine::process::GitCommand`: builder over `tokio::process`, always passes `--no-optional-locks -c core.quotepath=off`, sets `CREATE_NO_WINDOW` on Windows, `GIT_TERMINAL_PROMPT=0`, timeout, captures stdout/stderr as bytes, `-z` split helper. Tests: timeout fires; NUL split handles empty fields; exit code mapping to `GitError`. · model: opus
- [ ] **P0-07** macOS login-shell environment: resolve `PATH` once via the user's shell (`$SHELL -ilc 'echo $PATH'`) with a 2 s timeout and fallback; used by `GitCommand`. Test on macOS runner. · model: opus
- [ ] **P0-08** `git_engine::status`: run `git status --porcelain=v2 -z --branch --untracked-files=all` and parse into `RepoInfo` + `Vec<StatusEntry>` (ordinary, renamed/copied, unmerged, untracked, ignored). Fixture script `scripts/fixtures/basic.sh` creates a repo with each entry kind. Tests for all kinds incl. rename with spaces and unicode path. · model: opus
- [ ] **P0-09** `RepoActor`: tokio task per repo owning `Repo` handle, mailbox for commands, serialises writes, concurrent reads. `open_repo(path)` discovers `.git` (incl. worktree `.git` file). Test: two concurrent reads, one write, ordering preserved. · model: opus
- [ ] **P0-10** File watcher: `notify` recursive watch on worktree + `.git` (HEAD, index, refs/, packed-refs, *_HEAD, rebase-merge/, rebase-apply/, logs/), respects `.gitignore` via `ignore` crate, coalesces into `RepoChanged { kinds }` after 150 ms (250 ms Windows). Own-write suppression via generation counter. Tests: touch file → one event; edit `.git/HEAD` → `kinds` contains `head`. · model: best
- [ ] **P0-11** Settings store: `settings.json` in Tauri `app_config_dir`, typed `Settings` struct, recent repos list (max 20), git binary override, atomic write. Tests: round-trip, corrupt file recovers to defaults. · model: opus
- [ ] **P0-12** Tauri commands `open_repo`, `list_recent_repos`, `get_status`; events `repo-changed`. UI: minimal window with "Open repository" (native dialog) and a raw list of status entries that refreshes on `repo-changed`. · model: opus
- [ ] **P0-13** `git-engine-cli`: `status <path>` and `watch <path>` subcommands printing JSON, for manual testing. · model: opus
- [ ] **P0-14** Release workflow skeleton `release.yml` (tag `v*`, build matrix for 5 targets, artifacts uploaded, signing steps present but skipped when secrets are absent). Dry run on a `v0.0.1-test` tag. · model: opus
- [ ] **P0-15** Repo hygiene: `.editorconfig`, `rustfmt.toml`, `.prettierrc`, `LICENSE-THIRD-PARTY` generation via `cargo about` + `license-checker` (stub), `CONTRIBUTING.md` pointing at CLAUDE.md. · model: sonnet
- [ ] **P0-16** Landing-page waitlist stub: `site/` folder with a static page and a newsletter form (provider TBD), deployed to GitHub Pages. Marketing starts now (§10). · model: sonnet

## Exit criteria

- App opens a repo and renders live `git status` on macOS, Windows and Linux.
- Watcher reflects an external `touch` within 300 ms on all 3 OSes.
- CI matrix green; `pnpm gen:types` idempotent; release dry-run produces artifacts.
- No `unwrap()` outside tests; clippy clean.
