# Phase 5 — GitLab (3 weeks, ends 2027-02-14)

Spec: §3 H1–H9 (GitLab side), L0; §7 Authentication (GitLab), User flows, Line-position mapping; Appendix A11.
Goal: MR lifecycle on gitlab.com and a self-managed instance with the same UI as GitHub.

## forge-gitlab

- [ ] **P5-01** OAuth 2.0 PKCE: loopback server on `127.0.0.1:<random>`, `code_verifier/challenge`, `state` check, browser launch, `POST /oauth/token`, refresh-token handling (2 h expiry), `GET /api/v4/user`. Tests with mock server incl. refresh on 401. · model: opus
- [ ] **P5-02** Personal access token auth: paste + validate; stored like OAuth; self-managed host field. · model: opus
- [ ] **P5-03** HTTP client with rate-limit headers, pagination (`Link`/`X-Next-Page`), project id resolution from `owner/repo` (URL-encoded path). · model: opus
- [ ] **P5-04** MR list (`merge_requests?state=opened&…`) + per-MR approvals and head pipeline → `PrSummary`; filters (author, assignee, reviewer, labels, state). Recorded-fixture tests. · model: opus
- [ ] **P5-05** MR detail: description, commits, changes/diffs with `diff_refs` (base/start/head sha), discussions with `position`, approvals, pipelines and jobs; `pr_fetch_refspec` = `merge-requests/<iid>/head`. · model: opus
- [ ] **P5-06** Create MR (title, description from `.gitlab/merge_request_templates/`, target, draft, reviewers, assignees, labels, remove-source-branch, squash), approve/unapprove, discussions (`POST /discussions` with `position { base_sha, start_sha, head_sha, old_path, new_path, old_line, new_line }`), reply, resolve, merge (`squash`, `should_remove_source_branch`, `merge_when_pipeline_succeeds`). Tests incl. 422 message surfacing. · model: opus
- [ ] **P5-07** Capabilities: `approval_rules`, `merge_when_pipeline_succeeds`, `squash` from project settings; UI hides what is missing. · model: opus
- [ ] **P5-08** Pipelines for a sha → `CheckRun` list; job log URL; retry job (optional). · model: opus
- [ ] **P5-09** Provider detection for self-managed: probe, user mapping UI; GitLab repo browser for Clone-from-account (groups + projects, search). · model: opus
- [ ] **P5-10** Credential helper support for GitLab hosts (token as password, `oauth2` username for OAuth tokens). · model: opus

## App + UI

- [ ] **P5-11** Accounts settings: Sign in with GitLab (PKCE flow with "waiting for browser" state, cancel), PAT alternative, custom host; multiple accounts. · model: sonnet
- [ ] **P5-17** GitLab SSH key onboarding: the P4-27 flow for gitlab.com and self-managed hosts — "Add to GitLab account" via `POST /user/keys` (`title`, `key`), Test connection (`ssh -T git@<host>`); shown in Onboarding and Settings → SSH keys when a GitLab account is signed in. Tests against a mock server. · model: opus
- [ ] **P5-12** Terminology and capability adaptation: labels ("Merge request", "Approve"), hidden controls per `Capabilities`; parity checklist doc `docs/plans/forge-parity.md` filled for GitHub vs GitLab. · model: sonnet
- [ ] **P5-13** Review mapping: internal comment positions → GitLab `position`; multi-line ranges via `line_range`; tests with a recorded MR diff. · model: sonnet
- [ ] **P5-14** Version + build-date stamp: embedded at build (`BUILD_DATE`, semver) shown in About and included in updater manifest (§3 L0). No license code. · model: sonnet
- [ ] **P5-15** About screen with third-party licenses (generated), version, build date, links. · model: sonnet
- [ ] **P5-16** Marketing: GitLab MR review guide draft; newsletter #5; post in GitLab forum. · model: sonnet

## Exit criteria

- Full MR lifecycle on gitlab.com and a self-managed instance (Docker GitLab CE in CI or a hosted test instance) on all 3 OSes.
- Parity checklist: every GitHub PR flow has a GitLab equivalent or a documented capability gap.
- Token refresh on 401 verified; PAT path verified.
- About shows version and build date; no licensing code exists in the tree.
