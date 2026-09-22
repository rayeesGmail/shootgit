# Phase 4 — GitHub integration (4 weeks, ends 2027-01-24)

Spec: §3 H1–H9; §7 all; Appendix A11.
Goal: full PR lifecycle (create, review, merge) on github.com and a GHES-style custom host, working offline from cache.

## forge-core

- [ ] **P4-01** `Forge` trait, neutral models (`RepoMeta`, `PrSummary`, `PrDetail`, `ReviewSubmission`, `MergeOptions`, `CheckRun`, `Capabilities`), `AuthStrategy` trait, `ForgeError` with `RateLimited { reset_at }`, `Unauthorized`, `NotFound`, `Validation(msg)`. · model: best
- [ ] **P4-02** Provider detection: parse SSH/HTTPS/`ssh://` remote URLs into `RemoteIdentity`; known hosts map; probe unknown hosts (`/api/v4/version`, `/api/v3/meta`) with 3 s timeout; persisted user mapping. Tests with 20 URL shapes. · model: opus
- [ ] **P4-03** SQLite cache (`rusqlite`, bundled): schema for repo meta, PR summaries, PR details, checks, ETags; `CacheStore` API; migration runner. Tests. · model: opus
- [ ] **P4-04** Polling scheduler: per-repo session with window-focus and manual triggers, intervals (PR list 90 s, HEAD checks 20 s while focused), backoff on rate limit, cancellation on repo close. Tests with mocked clock. · model: opus
- [ ] **P4-26** Idle discipline: PR polling pauses when the window is unfocused or on battery-saver; forge session starts after first paint at low priority; SQLite cache capped at 50 MB per repo with LRU eviction. Test: idle CPU 0 % over 5 min with forge connected. · model: opus
- [ ] **P4-05** Keychain accounts: `AccountStore` over `keyring` with keys `forge/<provider>/<host>/<user_id>`; multiple accounts; account ↔ remote matching. Tests with an in-memory keyring backend. · model: opus

## forge-github

- [ ] **P4-06** Device flow auth: `/login/device/code` → poll `/login/oauth/access_token` honouring `interval` and `slow_down`; scopes `repo read:org workflow notifications`; `GET /user`; GHES base-URL support. Tests against a mock server (wiremock). · model: opus
- [ ] **P4-07** HTTP client: `reqwest` with `X-GitHub-Api-Version` pin, auth header, ETag/If-None-Match, rate-limit header parsing, retry on 5xx. Tests. · model: opus
- [ ] **P4-08** GraphQL PR list query (number, title, author, draft, labels, reviews decision, `statusCheckRollup`, mergeable, updatedAt, head/base refs) with pagination; map to `PrSummary`. Recorded-fixture tests. · model: opus
- [ ] **P4-09** PR detail (REST + GraphQL): body, commits, files, review threads with positions, checks; `pr_fetch_refspec` = `pull/<n>/head`. Tests. · model: opus
- [ ] **P4-10** Create PR (title, body from `.github/PULL_REQUEST_TEMPLATE.md`, base, draft, reviewers, labels), merge (`merge|squash|rebase`, delete branch), review submission (`POST /pulls/<n>/reviews` with comments `path/line/side/start_line` and event). Validation errors surfaced verbatim. Tests. · model: opus
- [ ] **P4-11** Checks for a sha (check runs + legacy statuses) → `CheckRun` list; open-in-browser URLs for repo, commit, file@line. · model: opus
- [ ] **P4-12** Repo browsing for Clone-from-account (user + org repos, search). · model: opus

## credential-helper

- [ ] **P4-13** `credential-helper` `get`/`store`/`erase` for forge hosts: on `get` returns the matching signed-in account's token as `password` and login as `username`; wired per invocation with `-c credential.helper=<path>` on git spawns whose remote host matches a signed-in account, so the user's own helpers (GCM, osxkeychain, libsecret) stay first in Git's order and no config file is touched. Tests: helper protocol round-trip; git push to a mock HTTPS remote succeeds with the OAuth token; a host with no account never sees the helper. · model: opus

## App + UI

- [ ] **P4-14** Tauri commands: `forge_login_github`, `forge_accounts`, `forge_logout`, `forge_list_prs`, `forge_pr`, `forge_create_pr`, `forge_review`, `forge_merge`, `forge_checks`; events `forge-updated`, `forge-error`. · model: sonnet
- [ ] **P4-15** Accounts settings: Sign in with GitHub (device-code screen: code, copy, open browser, waiting state), GHES host field, multiple accounts, sign out. · model: sonnet
- [ ] **P4-27** SSH key onboarding: Generate ed25519 key (passphrase optional, stored via P1-25), Copy public key, "Add to GitHub account" via `POST /user/keys`, Test connection; shown in Onboarding when no key exists and in Settings → SSH keys. GitLab equivalent (`POST /user/keys`) lands in P5-17. · model: opus
- [ ] **P4-16** Pull Requests view: list tabs (Mine, Review requested, All), filters (state, author, label), CI dot, draft badge, stale/offline indicator, manual refresh. · model: sonnet
- [ ] **P4-17** PR detail: description (markdown rendered), commits, checks list with links, reviewers and decision, timeline of review threads; actions: Check out, Open in browser, Copy link. · model: sonnet
- [ ] **P4-18** Local review: on Check out, compute base..head diff locally; file list with review diff; add line/range comments (pending, stored locally in SQLite), reply in threads, resolve; Submit review (Comment / Approve / Request changes) posts batch. Map internal old/new line numbers to GitHub `line/side`. · model: opus
- [ ] **P4-19** Create PR dialog from branch: push with `-u` if needed, base picker, title/body with template, draft, reviewers, labels; success links branch to PR. · model: sonnet
- [ ] **P4-20** Merge dialog: allowed strategies from repo settings, delete-branch option; after merge: fetch, offer local branch delete, switch to base. · model: sonnet
- [ ] **P4-21** CI badges in Log rows and PR header; click opens check details/URL. · model: sonnet
- [ ] **P4-22** Clone from account: repo browser with search in Onboarding/Clone dialog. · model: sonnet
- [ ] **P4-23** Open on GitHub / Copy link to file or line actions in Changes and Log. · model: sonnet
- [ ] **P4-24** Offline behaviour: all PR panels render from cache with "updated Xh ago"; write actions disabled with reason. · model: sonnet
- [ ] **P4-25** Marketing: PR review demo; newsletter #4; open GitHub Discussions for feedback. · model: sonnet

## Exit criteria

- Full lifecycle on a test org: sign in → list → check out → review with inline comments → approve → merge → local cleanup, on all 3 OSes.
- Same flow against a GHES-style custom host (mock) via provider detection.
- Airplane-mode test: PR list and details render from cache; no spinners.
- Credential helper makes HTTPS push work with the OAuth token.
- H1–H9 verified.
