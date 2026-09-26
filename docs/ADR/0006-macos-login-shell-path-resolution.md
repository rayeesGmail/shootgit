# 0006 — Resolve the macOS login-shell PATH via `git_engine::process`, not `fix-path-env`

- Status: accepted
- Date: 2026-09-26
- Spec: overrides §9 (Cross-platform table, "Process spawn" row for macOS)

## Context

An app launched from Finder or the Dock on macOS inherits launchd's minimal
`PATH` (`/usr/bin:/bin:/usr/sbin:/sbin`), not the one the user's shell builds
in `.zshrc`/`.bash_profile`, so a Homebrew-installed `git` or `ssh` would be
invisible. §9's Cross-platform table names the fix as "Login-shell env via
`fix-path-env`" — a third-party crate that asks `$SHELL -ilc` for the
environment.

That crate spawns the shell with raw `std::process`, not through
`git_engine::process`. CLAUDE.md's architecture and performance rules require
every spawn in our code to go through that module: `GitCommand` for git and
the generic `ProcessCommand` builder for everything else, so timeouts,
`CREATE_NO_WINDOW`, the shared concurrency limiter and the spawn counter
(P0-06, P0-17) all apply, and so the P0-18 perf gate's spawn-count budget
stays accurate. Adopting `fix-path-env` as specified would add a spawn path
outside that module, invisible to the limiter and the counter, and blocking
rather than async — the same category of gap P0-07's own follow-up work
(the synchronous `git_binary::run()` probes) was fixing.

This came up while implementing P0-07 (macOS login-shell PATH resolution).

## Options considered

1. **Use `fix-path-env` as the spec's table names it** — matches the spec
   text exactly; the crate is used elsewhere in the Tauri ecosystem for this
   exact problem and imports the whole login environment (`PATH`,
   `SSH_AUTH_SOCK`, `LANG`, etc.), not just `PATH`. But it spawns the shell
   itself, outside `git_engine::process`, so it bypasses the shared limiter,
   the spawn counter, and our timeout/cancellation model; it is also
   synchronous, which does not fit the single-runtime, no-ad-hoc-thread rule
   without wrapping it in a blocking-pool call.
2. **Hand-roll the `$SHELL -ilc` lookup inside `git_engine::process`** — a
   new `login_shell` module builds the same `-ilc` invocation on top of
   `ProcessCommand`: a 2 s timeout, one shell run cached for the life of the
   process, and a fallback to the process's own `PATH`. It reuses the exact
   builder, spawn counter and limiter plumbing P0-06/P0-17 already built, and
   is testable with the same fake-`$SHELL`-script fixtures P0-05 uses. Its
   cost is narrower scope — it imports only `PATH`, not the rest of the login
   environment — and it is code we own and maintain instead of a crate.

## Decision

Option 2. `git_engine::login_shell` resolves `PATH` by spawning
`$SHELL -ilc` through `ProcessCommand`, so the probe is timed, cancellable,
counted as a spawn, and — unlike `fix-path-env` — sits inside the same
spawn surface as every other process the app starts. This keeps the
"every spawn goes through `git_engine::process`" rule intact at the cost of
not importing the rest of the login environment the way `fix-path-env`
would.

## Consequences

- Spawn accounting (the perf harness, P0-18's constrained-VM budget) stays
  accurate: the shell probe is a counted, limited spawn like any other, not
  an invisible one from a third-party crate.
- The resolver is testable in-tree with the fake-shell fixtures already used
  for P0-05/P0-06, without depending on how a third-party crate chooses to
  test itself.
- Only `PATH` is imported. P1-25's `ssh`/`ssh-add`/`ssh-keygen` work (which
  may want `SSH_AUTH_SOCK` from the login environment) cannot lean on
  `fix-path-env` for that and must extend `login_shell` or add its own
  narrow import, following the same through-`git_engine::process` rule.
- §9's Cross-platform table names a crate we are not using; it needs
  correction in the Claude Doc (see reminder below) so the spec no longer
  points at code the app doesn't run.
