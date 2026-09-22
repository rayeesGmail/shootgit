---
description: Gap review of the current phase's exit criteria against the code
argument-hint: "[phase number]"
# Pinned to `best` (CLAUDE.md): Fable 5.1 if available, else Opus 5. There
# is no automatic fallback; if Fable is unavailable, change this to `opus`.
model: fable
effort: xhigh
---

Review phase `$ARGUMENTS` (default: the lowest-numbered
`docs/plans/phase-N.md` with an unchecked task) against its exit criteria.
This is a read-only review: do not edit, tick, commit or spawn subagents.

1. Read the phase file, its exit criteria, and the matching phase in
   `docs/SPEC.md` §10 plus the § sections its tasks cite.
2. Run the full suite from CLAUDE.md → Testing and `cargo fmt --all --check`,
   and read the CI workflow files to see what runs on which OS.
3. For every exit criterion, find the evidence (test names, code paths, CI
   jobs) and classify it:
   - **Met**: verified by a passing test or CI job on all three OSes;
   - **Met locally**: passes here, other OSes unverified;
   - **Partly met** or **Not met**: say what is missing;
   - **Not testable yet**: say what would make it testable.
4. List unchecked tasks, checked tasks whose tests are missing or skipped,
   conflicts between code, plan and spec, and CLAUDE.md rules the code
   breaks (unwrap outside tests, human-readable git parsing, spawns outside
   `GitCommand`, unbounded collections, polling timers).

Output one table of criteria (criterion · status · evidence), then the
lists, then a short verdict: can the next phase start?
