---
description: Draft an ADR for a decision made in this session
argument-hint: "<decision title>"
model: sonnet
---

Draft an ADR for the decision `$ARGUMENTS` (if empty, the most recent
architectural decision in this conversation; ask if it is ambiguous).

1. Number it one above the highest `docs/ADR/NNNN-*.md` and name the file
   `docs/ADR/NNNN-kebab-title.md`.
2. Follow `docs/ADR/0000-template.md` exactly. Status is `proposed` unless
   the user has said the decision is accepted. Date is today. Fill the
   `Spec:` line with the § sections it refines or overrides.
3. Take Context, Options, Decision and Consequences from what was actually
   discussed in this session. Do not invent options nobody considered; if
   there was only one real option, say so.
4. Show the draft and list any other files that should change with it (plan
   tasks, CLAUDE.md rules). Change them only if the user agrees.
5. Remind the user to add the decision to the §12 Decisions log in the
   Claude Doc and re-export; `docs/SPEC.md` is never hand-edited.

Do not commit.
