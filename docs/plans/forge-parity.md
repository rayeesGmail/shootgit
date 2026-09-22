# Forge parity checklist (GitHub vs GitLab)

Fill during Phase 5 (P5-12). Every row must be "Same", "Adapted (how)", or "Gap (capability flag)".

| Flow | GitHub | GitLab | Status |
| --- | --- | --- | --- |
| Sign in | Device flow | PKCE / PAT | |
| Multiple accounts | | | |
| Self-hosted host detection | GHES `/api/v3/meta` | `/api/v4/version` | |
| List PRs/MRs + filters | | | |
| Detail: description, commits, checks, reviews | | | |
| Check out locally | `pull/<n>/head` | `merge-requests/<iid>/head` | |
| Inline comments (single line) | `line/side` | `position` | |
| Inline comments (range) | `start_line` | `line_range` | |
| Approve / request changes | review event | approve / no equivalent | |
| Create PR/MR with template | `.github/PULL_REQUEST_TEMPLATE.md` | `.gitlab/merge_request_templates/` | |
| Draft | | | |
| Merge strategies | merge/squash/rebase | merge/squash (+ when pipeline succeeds) | |
| Delete source branch | | | |
| CI status per commit | check runs + statuses | pipelines + jobs | |
| Open in browser / copy link | | | |
| Clone from account | | | |
| Issues list/create/link | | | |
