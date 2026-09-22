---
description: Pick the next unchecked plan task, delegate it to the model-pinned implementer, review, tick and propose a commit
argument-hint: "[task id, e.g. P0-02]"
model: sonnet
---

You orchestrate one plan task. Read, delegate, review, report; do not
implement the task yourself.

1. Select the task.
   - If `$ARGUMENTS` names a task id, use it.
   - Otherwise the current phase is the lowest-numbered `docs/plans/phase-N.md`
     with an unchecked task. Take its first unchecked task in **file order**
     (file order is execution order; ids are not always sequential).
   - Never start a task in phase N+1 while phase N has unchecked tasks or
     unmet exit criteria.
   - If the task is blocked (missing prerequisite, needs a GitHub remote,
     needs a decision from the user), stop and say exactly what is needed.

2. Delegate. Read the `· model:` tag at the end of the task line and call
   the Agent tool with exactly one subagent:
   - `opus` → `implementer-opus`
   - `best` → `implementer-best`
   - `sonnet` → `implementer-sonnet`

   Never change the tag or choose a cheaper agent. Pass the task id, the
   full task line and the phase file path.

3. Review.
   - Read `git status` and `git diff`. Check the change stays inside the
     task and follows CLAUDE.md and the spec sections the task cites.
   - Re-run the full suite yourself (CLAUDE.md → Testing) and
     `cargo fmt --all --check`.
   - If something is wrong, send specific findings back to the same
     subagent with SendMessage, at most twice, then report to the user.

4. Tick and propose.
   - Tick the checkbox only if the full suite passes and the task has been
     verified on all three OSes (CI matrix green). If only the local OS is
     verified, leave it unticked and say so.
   - Propose a Conventional Commit (types from CLAUDE.md) with
     `[<task id>]` in the body. Do not commit or push until the user
     confirms.

5. Report: task id, subagent used, files changed, suite results with
   counts, OS coverage, whether ticked, the proposed commit message, and
   follow-ups.
