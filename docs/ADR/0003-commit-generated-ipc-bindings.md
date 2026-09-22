# 0003 — Commit generated IPC bindings

- Status: accepted
- Date: 2026-09-22
- Spec: refines §4 IPC contract

## Context
`pnpm gen:types` generates `packages/ipc-types/bindings.ts` from the Rust command signatures via specta. The TypeScript toolchain (editor, typecheck, Vitest) must be able to use these bindings without cargo, and changes to the IPC surface should show up in PR diffs.

## Options considered
1. Don't commit; regenerate in CI and typecheck against the result — keeps generated output out of the tree; the frontend can no longer be edited or checked without a Rust toolchain, and API changes are hidden from review.
2. Commit the file and verify freshness in CI with `pnpm gen:types && git diff --exit-code packages/ipc-types/bindings.ts` — frontend works without Rust and IPC changes are reviewable; contributors must remember to regenerate.

## Decision
Option 2. `bindings.ts` is committed and never hand-edited, and CI fails when it differs from freshly generated output (P0-04). This is the one exception to the "do not commit generated files" rule in `CLAUDE.md`.

## Consequences
Contributors must run `pnpm gen:types` after changing any command signature and commit the result; CI enforces it. PRs that touch the IPC surface show the TypeScript change alongside the Rust change.
