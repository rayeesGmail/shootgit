# 0007 — Clear inherited repository-local git variables on every git spawn

- Status: accepted
- Date: 2026-09-26
- Spec: refines §4 (Process model), §5

## Context

P0-06 left a follow-up for P0-09: an inherited `GIT_DIR`, `GIT_INDEX_FILE`
or `GIT_WORK_TREE` would redirect every `GitCommand`. Git exports `GIT_DIR`
and `GIT_INDEX_FILE` to hooks, so a hook that starts the app passes them on.
Some users also export `GIT_DIR` in their shell, for example to manage
dotfiles in a bare repository.

P0-09's `open_repo` finds the repository that contains a path and runs git
in that working tree. An inherited `GIT_DIR` makes git ignore the `.git` in
its working directory and use another repository. At best status shows the
wrong repository. At worst a commit goes into it, or staging goes into a
hook's temporary index.

## Options considered

1. **Leave the environment as inherited.** No code, but every git spawn in
   an app started from a hook or from such a shell works on the wrong
   repository.
2. **Set `GIT_DIR` and `GIT_WORK_TREE` on every spawn to the paths
   `open_repo` found.** This overrides the two main variables. It leaves
   `GIT_INDEX_FILE`, `GIT_OBJECT_DIRECTORY`, `GIT_COMMON_DIR` and the others
   in place. With an explicit `GIT_DIR`, git also skips its `safe.directory`
   ownership check (the CVE-2022-24765 protection). A repository owned by
   another user could then run its config (`core.fsmonitor`, hooks) as the
   current user.
3. **Clear git's repository-local variables on every spawn.** These are the
   variables `git rev-parse --local-env-vars` lists. A value the caller sets
   on the command still applies. Git clears the same list before it runs a
   command in a submodule. Git then finds the repository from its working
   directory as usual, with `safe.directory` still enforced.

## Decision

Option 3. On every spawn, `GitCommand` removes each variable in
`git_engine::process::LOCAL_REPO_ENV` from the environment it inherited.
There are 16 names: git 2.39's list, which still includes
`GIT_INTERNAL_SUPER_PREFIX`, dropped in 2.40. `GitCommand::env` can still
set one of them on purpose. P6-01, for example, commits a changelist through
a temporary `GIT_INDEX_FILE`. `open_repo` never reads the environment.

## Consequences

- An app started from a hook or from a `GIT_DIR` shell opens the repository
  the user picked.
- The app cannot be pointed at a repository through `GIT_DIR`, for example
  a bare dotfiles repository with a separate working tree. That would need
  `core.worktree` support in `open_repo` and is out of scope for 1.0.
- `GIT_CONFIG_PARAMETERS` and `GIT_CONFIG_COUNT` are cleared as well. So
  `-c` options exported by a parent git, and `GIT_CONFIG_KEY_<n>` /
  `GIT_CONFIG_VALUE_<n>` pairs from the user's shell, do not reach the git
  the app runs. Config comes only from config files and the per-spawn `-c`
  list.
- Other tools spawned through `ProcessCommand` (`ssh` and friends) keep the
  inherited environment.
- The list has to track git's. The unit test
  `cleared_env_covers_what_this_git_calls_repository_local` fails when the
  git on a CI runner lists a variable that `LOCAL_REPO_ENV` does not.
- The decision needs a row in the §12 Decisions log (Claude Doc, then
  re-export; `SPEC.md` is never hand-edited).
