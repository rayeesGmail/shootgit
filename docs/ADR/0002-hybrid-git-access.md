# 0002 — Hybrid Git access: CLI for writes, gitoxide for reads

- Status: accepted
- Date: 2026-09-19
- Spec: refines §5

## Context
Writes must respect the user's config, hooks, signing, credential helpers and LFS. Reads must be fast, especially on Windows where process spawn is slow.

## Options considered
1. CLI only — correct, simple; slow reads on Windows, many spawns.
2. libgit2 (git2) only — fast; reimplements hooks/signing/LFS poorly; GPLv2 with linking exception.
3. gitoxide only — fast, pure Rust; write paths not mature enough.
4. Hybrid — CLI for writes, gix for reads with CLI fallback.

## Decision
Hybrid. Every read path has a CLI fallback so a gix gap never blocks a feature.

## Consequences
Two code paths for reads must agree; fixture tests run both. Pin gix versions.
