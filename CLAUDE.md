# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

This repository is a cross-platform Git client (macOS, Windows, Linux) with
IntelliJ-grade Git tooling and GitHub/GitLab integration, built with Tauri 2,
a Rust core and a SolidJS/TypeScript frontend. 1.0 ships free.

The authoritative spec is `docs/SPEC.md`. Section numbers below (§n) refer to
it. Read the relevant section before any non-trivial change. If code and spec
disagree, the spec wins unless an ADR in `docs/ADR/` says otherwise.
`SPEC.md` is a Markdown export of a Claude Doc (see `docs/README.md`); never
hand-edit it. If it is missing, ask the user to export it rather than guessing
what a § says.

## Bootstrap status (delete this section once Phase 0 is done)

As of 2026-09-22 only the Cargo workspace skeleton exists (P0-01: six empty
lib crates under `crates/`, toolchain pinned in `rust-toolchain.toml`):
- `src-tauri`, `packages/`, `scripts/` and CI do not exist yet. Paths and
  commands below that refer to them are the target layout the rest of Phase 0
  creates; a command fails until the task that introduces it has landed
  (`pnpm test` and `pnpm typecheck` arrive with P0-03).

## How work is organised

- Tasks live in `docs/plans/phase-N.md` as checkboxes. Work on ONE unchecked
  task per session, in order, unless told otherwise.
- Every task is test-first: write the failing test, implement, run the full
  suite, then tick the box and commit.
- Phase exit criteria are at the bottom of each plan file and in §10. Do not
  start the next phase until the current exit criteria pass on all three OSes.
- Architectural choices that are not already in the spec get an ADR
  (`docs/ADR/NNNN-title.md`, template in `docs/ADR/0000-template.md`).

## Architecture rules (§4)

- The frontend NEVER spawns git, reads the filesystem, or calls the network.
  Everything goes through Tauri commands defined in `src-tauri/src/commands/`.
- Git WRITES use the `git` CLI. Git READS prefer `gix` (gitoxide) with a
  transparent CLI fallback (§5). Never link libgit2.
- Every Tauri command has a specta-generated TypeScript type in
  `packages/ipc-types/`. Run `pnpm gen:types` after changing any Rust command
  signature. Never hand-edit generated files.
- Long operations return an operation id and stream `op-progress`,
  `op-done`, `op-error` events. Repo state changes emit ONE coalesced
  `repo-changed` event (150 ms debounce, 250 ms on Windows).
- One `RepoActor` per open repository serialises writes; reads run
  concurrently.
- Parse ONLY machine-readable git output: `--porcelain=v2 -z`,
  `--format=... -z`, `--patch`. Never parse human-readable output.
- `GitCommand` always passes `--no-optional-locks -c core.quotepath=off` and
  sets `GIT_TERMINAL_PROMPT=0`. Minimum git is 2.30; the binary is resolved
  as settings override → PATH → bundled (P0-05).
- Each gix read has a CLI fallback that must return the same result.
  Fixture tests run both paths, and gix versions are pinned (ADR 0002).
- App-private per-repo state lives under `.git/<app>/` (`safety/`,
  `shelves/`, `messages.json`, `changelists.json`), never in the worktree.
- The app binary doubles as git's editor. `<app> --seq-editor <file>` and
  `<app> --msg-editor <file>` (passed as `GIT_SEQUENCE_EDITOR` and
  `GIT_EDITOR`) write the prepared todo or message and exit 0 without opening
  a window. Interactive rebase, reword and squash are built on this (P3-05).
- GitHub and GitLab both implement `forge_core::Forge` over neutral models.
  Provider differences are expressed as `Capabilities`, and the UI hides what
  a provider lacks. PR/MR data is cached in SQLite so panels render offline;
  while offline, write actions are disabled and show the reason.
- Keychain entries are keyed `forge/<provider>/<host>/<user_id>`. The
  credential helper is registered through the app-owned gitconfig include,
  and only after the user consents.

## Crate and package map

| Path | Purpose |
| --- | --- |
| `crates/git-engine` | All local Git logic. Pure library, no Tauri, no UI. |
| `crates/git-engine-cli` | Dev CLI over git-engine for manual testing. |
| `crates/forge-core` | `Forge` trait, neutral models, auth traits, SQLite cache. |
| `crates/forge-github` | GitHub REST + GraphQL, device-flow auth. |
| `crates/forge-gitlab` | GitLab REST, PKCE + PAT auth. |
| `crates/credential-helper` | Separate binary implementing `git credential`. |
| `src-tauri` | Tauri app: commands, events, menus, settings, updater. |
| `packages/ipc-types` | Generated TS types. Committed, never hand-edited; regenerate with `pnpm gen:types`. |
| `packages/ui` | Solid app: `views/`, `components/`, `stores/`, `lib/`. |
| `scripts/` | Fixture builders, release helpers. |

## Conventions

Rust
- Edition 2021, `cargo clippy --all-targets -- -D warnings` clean,
  `cargo fmt` clean.
- Errors: `thiserror` enums per crate; no `anyhow` in library crates.
- Logging: `tracing`; never `println!` outside `git-engine-cli`.
- No `unwrap()` / `expect()` outside tests and `main.rs`. Enforced by
  `[workspace.lints.clippy]`; `main.rs` and each file under a crate's
  `tests/` start with `#![allow(clippy::unwrap_used, clippy::expect_used)]`.
- Public functions in `git-engine` take `&Repo` and return `Result<T, GitError>`.
- Process spawning only via `git_engine::process::GitCommand` (handles
  `CREATE_NO_WINDOW`, login-shell PATH on macOS, timeouts, `-z` parsing).

TypeScript
- `strict: true`, no `any`, no non-null assertions without a comment.
- State in `packages/ui/src/stores/` using Solid stores/signals; components
  are presentational and receive data via props or store hooks.
- Styling via CSS custom properties in `packages/ui/src/styles/`; no inline
  colours; respect `prefers-color-scheme` and `prefers-reduced-motion`.
- Diff/merge editors use CodeMirror 6; the commit graph is `<canvas>`.
- All user-visible strings go through `t()` from `packages/ui/src/i18n/`.

Git hygiene
- Conventional Commits (`feat:`, `fix:`, `perf:`, `test:`, `refactor:`,
  `docs:`, `chore:`); one logical change per commit; reference the plan task
  (`[P1-07]`) in the commit body.

## Testing

- `git-engine` behaviour needs a fixture-repo test in
  `crates/git-engine/tests/`. Fixtures are built by `scripts/fixtures/*.sh`
  and cover CRLF files, long paths, unicode names, submodules, LFS pointers,
  conflicts, in-progress rebases, and a 100k-commit synthetic history.
- Patch construction (line-level staging) uses golden tests:
  `tests/golden/<case>.selection.json` → `tests/golden/<case>.patch`.
- UI logic (stores, graph layout, keyboard map) is unit-tested with Vitest.
- End-to-end scenarios are shell scripts in `scripts/e2e/`. Criterion benches
  in `benches/` produce the nightly perf numbers checked against §1 budgets.
- Before declaring any task done run:
  `cargo test --workspace && cargo clippy --all-targets -- -D warnings && pnpm test && pnpm typecheck`
- A test that passes on one OS only is a bug, not a pass. If you cannot run
  the other OSes locally, say so explicitly and leave the task unticked until
  CI confirms.

## Safety rules (§5)

- Before hard reset, discard, force push, rebase, or checkout with a dirty
  tree, record a `SafetyPoint` (reflog head + optional shelf) so the
  operation is undoable.
- Force push only as `--force-with-lease`; plain `--force` is behind an
  advanced setting and blocked on protected branches.
- Never modify the user's global `~/.gitconfig`. App-specific config goes in
  the app's own include file and only with explicit user consent.
- Tokens and secrets live in the OS keychain via the `keyring` crate. Never
  in JSON, SQLite, logs, or test fixtures.
- Byte-preserve line endings in every patch; never normalise EOL.

## Performance budgets (§1, §6)

- Cold start to usable Changes view: < 1.0 s on a 50k-file repo.
- Status refresh after a file save: < 300 ms.
- Line-level stage/unstage: < 100 ms round-trip.
- Log scroll on 500k commits: 60 fps.
- Download < 25 MB; RAM < 150 MB on a mid-size repo.
- On Windows, minimise `git.exe` spawns: batch, cache, or use `gix`.
- Low-end laptop (8 GB, 2 cores, slow disk) is a hard target (§4 Low-resource operation): every budget within 2×, peak RSS < 250 MB on a mid-size repo, idle CPU 0 %, no main-thread stall > 100 ms.
- Lazy by default: nothing computes for a view that is not visible or a repo that is not active. Cancel in-flight reads when the request is superseded.
- Bounded memory: Log rows are an LRU window, big diffs load hunk-by-hunk, blobs stream. Never hold a whole history or a whole large file in memory.
- All `git` spawns in our code go through the shared limiter in `git_engine::process`, and all async work runs on the single runtime handed to Tauri. Do not start ad-hoc threads or spawn `git` outside those paths. Threads that libraries manage internally (notify's watcher, the WebView, SQLite) are fine. No polling timers where a watcher or event exists.

## Cross-platform (§9)

- Test paths with spaces, unicode and > 260 chars; on Windows set
  `core.longpaths=true` when needed.
- Windows: `CREATE_NO_WINDOW` on every spawn; watcher debounce 250 ms.
- macOS: resolve login-shell PATH once at startup; never use the
  `/usr/bin/git` Xcode stub when CLT is absent; ship a universal binary.
- Linux: WebKitGTK 4.1; handle inotify watch limits with a polling fallback.
- Keyboard shortcuts use ⌘ on macOS and Ctrl elsewhere via the shortcut map
  in `packages/ui/src/lib/keymap.ts`.

## Do not

- Add Electron, a Node runtime, React, or any second UI framework.
- Add a licensing, trial, account, or feature-gating system. 1.0 is free
  (§3 L0). Only the version + build-date stamp in About is allowed. This
  holds until Phase 8 (`docs/plans/phase-8.md`) is explicitly triggered.
- Add telemetry that is on by default or that sends file contents, paths,
  or commit messages.
- Introduce network calls from `git-engine`.
- Commit generated files, fixtures' `.git` directories, or secrets. One
  exception: `packages/ipc-types/bindings.ts` IS committed, because the
  frontend must typecheck without a Rust toolchain and IPC changes must be
  visible in review. Never hand-edit it; run `pnpm gen:types` and commit the
  result. CI fails if it is stale (P0-04).
- Claim a task is complete if any test is skipped or any OS is unverified.

## Useful commands

```
pnpm install && cargo build            # first build
pnpm tauri dev                          # run the app
cargo test -p git-engine                # engine tests only
cargo test -p git-engine --test <file> <name> -- --nocapture  # one test
pnpm --filter ./packages/ui exec vitest run <path>            # one Vitest file
pnpm gen:types                          # regenerate IPC types
scripts/fixtures/build-all.sh           # (re)build fixture repos
cargo run -p git-engine-cli -- status <repo>   # poke the engine
```

## Model routing (automatic)

Every task in `docs/plans/*.md` ends with `· model: opus | sonnet | best`.
`/task` runs as a cheap Sonnet orchestrator and delegates the implementation
to the subagent pinned to that model:

| Tag | Subagent | Resolves to | Used for |
| --- | --- | --- | --- |
| `opus` | `implementer-opus` | Opus 5 | Rust engine, Tauri, forge clients |
| `best` | `implementer-best` (effort xhigh) | Fable 5.1 if available, else Opus 5 | Graph layout, three-way merge, patch builder, watcher, perf |
| `sonnet` | `implementer-sonnet` | Sonnet 5 | UI, styling, docs, fixtures, marketing |

Rules:
- Never change a task's model tag to make it cheaper without an ADR.
- If you are running inside a subagent, do not spawn further subagents.
- Do not set `CLAUDE_CODE_SUBAGENT_MODEL` in this repo; it overrides the
  per-agent pins (known issue with `inherit`).
- `/phase-review` is pinned to `best`; `/adr` to `sonnet`.
- Claude Code has no automatic model fallback. `implementer-best` and
  `/phase-review` pin `model: fable`; if Fable is unavailable, change both to
  `opus`. That is the documented fallback, not a downgrade, so it needs no ADR.
- Subagents and commands live in `.claude/agents/` and `.claude/commands/`;
  the implementers set `disallowedTools: Agent`.
- Project default model is `sonnet` (`.claude/settings.json`) because the
  main session mostly orchestrates. Switch with `/model opus` when you want
  to pair-program directly on engine code.

## Slash commands available in this repo

- `/task` — pick the next unchecked task, delegate it to the right
  model-pinned subagent, review the result, tick and propose a commit.
- `/phase-review` — check the current phase's exit criteria against the code
  (runs on `best`).
- `/adr` — draft an ADR for a decision made in this session.
