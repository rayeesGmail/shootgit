# Implementation plan

Eight phases, ~8 months, solo developer + Claude Code. Each phase file lists
session-sized tasks with stable ids (`P1-07`), the spec sections they
implement, and the phase's exit criteria. Work top to bottom; tick boxes only
when the full test suite passes.

| Phase | File | Target end | Focus |
| --- | --- | --- | --- |
| 0 | phase-0.md | 2026-10-04 | Foundation: workspace, Tauri shell, git spawn, watcher, CI |
| 1 | phase-1.md | 2026-11-01 | Changes view, line-level staging, commit, push/pull |
| 2 | phase-2.md | 2026-11-29 | Log graph, filters, commit actions, branch popup, stash |
| 3 | phase-3.md | 2026-12-27 | Merge tool, interactive rebase, shelve, undo |
| 4 | phase-4.md | 2027-01-24 | GitHub: auth, PRs, review, merge, CI, credential helper |
| 5 | phase-5.md | 2027-02-14 | GitLab parity, self-managed hosts, build stamp |
| 6 | phase-6.md | 2027-03-28 | Changelists, blame, history, a11y, perf, beta |
| 7 | phase-7.md | 2027-04-18 | Free launch: site, packages, docs, launch sequence |
| 8 | phase-8.md | when triggered | Monetization (deferred; see SPEC §8) |

Task id format: `P<phase>-<nn>`. Reference it in commit bodies.

Each task ends with `· model: opus | sonnet | best`; `/task` delegates to the subagent pinned to that model (see `/CLAUDE.md` → Model routing). Roughly: `opus` for Rust engine and forge work, `sonnet` for UI/docs/marketing, `best` (Fable 5.1 when available) for the ten or so hardest algorithmic tasks.

Rules of engagement are in `/CLAUDE.md`. Spec is `/docs/SPEC.md`.
