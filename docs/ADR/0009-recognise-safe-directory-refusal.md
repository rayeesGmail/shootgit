# 0009 — Recognise git's `safe.directory` refusal from its stderr

- Status: proposed
- Date: 2026-09-26
- Spec: refines §4 (Process model), §5 (Git engine), §7 (never modify the user's git config)

## Context

Since the CVE-2022-24765 fix (git 2.35.2, backported to 2.30.3), git refuses
to work in a repository owned by another user unless the user's global or
system config lists it in `safe.directory`. `open_repo` finds such a
repository on disk (P0-09). The first git command run in it then fails with
exit code 128 and a message. Shown raw, that message is a stderr dump. The
P0-09 follow-up asks for a clear error that names the fix,
`git config --global --add safe.directory <path>`, and the app must not run
that command itself.

CLAUDE.md says to parse only machine-readable git output. Git has no
machine-readable form of this refusal. The exit code, 128, is the one every
fatal error uses. The message is translated into the user's language unless
the locale is forced.

## Options considered

1. **Check ownership ourselves, before git runs.** Compare the owner of the
   working tree, `.git` and any `.git` file with the current user, and read
   `safe.directory` from the global and system config. This copies git's
   logic, including its root/`SUDO_UID` handling and its matching rules,
   which change between versions. On Windows it needs owner-SID comparisons
   through the Win32 security API, which cannot be verified on the
   developer's macOS machine.
2. **Recognise git's own refusal.** Run the spawns whose failures are
   classified with `LC_ALL=C`, so the message is English. Map exit code 128
   with "detected dubious ownership" (2.35.3 and later) or
   "unsafe repository" (the 2.30.3 to 2.35.2 backports) to
   `GitError::DubiousOwnership`. Git stays the judge of ownership.
3. **Show the raw stderr.** No code, but it is exactly the dump the
   follow-up rules out, and in the user's language.

## Decision

Option 2. `git_engine::repo::classify_failure` recognises the refusal, and
`Repo::classified_git_command` sets `LC_ALL=C` on the spawn. With the C
locale, gettext also ignores `LANGUAGE`. `status` uses both, and it is the
first git command the app runs in a repository it opens. The error carries
the working tree's path. `safe_directory_command` quotes that path for the
user's shell: single quotes on Unix, double quotes and forward slashes on
Windows, as git prints it. The app shows the command. It never runs it, and
the engine never writes git config.

This reads stderr, but only to classify a failure. The data git prints
(`--porcelain=v2 -z`) is still the only thing parsed, and it does not depend
on the locale. The P0-06 follow-up already expects classified spawns to force
the locale (owner P1-10).

## Consequences

- Opening a repository owned by another user fails with kind
  `dubious_ownership`, a plain message and the fix. The repository is not
  kept open or recorded as recent.
- If a future git rewords the message, the error falls back to
  `GitError::Failed` with git's stderr: still correct, just less helpful.
  `crates/git-engine/tests/dubious_ownership.rs` runs the real git on every
  CI OS (`GIT_TEST_ASSUME_DIFFERENT_OWNER=1`), so a rewording shows up there.
- Git commands other than `status` do not classify yet. P1-10, which owns
  locale forcing for classified spawns, should route them through
  `classified_git_command` and `classify_failure`.
- The decision needs a row in the §12 Decisions log (Claude Doc, then
  re-export; `SPEC.md` is never hand-edited).
