# Phase 6 — Polish & private beta (6 weeks, ends 2027-03-28)

Spec: §3 G11, G12, G13, G15, H10, N1, N5, N7, N8; §6 Design principles, Performance budgets; §9 Signing, Auto-update; Appendix A2 (changelists), A4, A5, A12.
Goal: parity items promoted to 1.0, accessibility and performance passes, signed builds, 100–150 beta testers.

## Features

- [ ] **P6-01** Changelists engine: metadata `.git/<app>/changelists.json` (name, comment, active, paths, hunk selections); create/rename/delete/set-active/move; Default list; commit-a-changelist = stash index → stage selection → commit → restore index. Tests incl. partial hunks across two lists. · model: opus
- [ ] **P6-02** Changelists UI: group-by toggle (changelist / directory / repository), drag files and hunks between lists, per-list commit, "Move to another changelist" in diff chunk menu; mode switch Changelists ↔ Staging area persisted per repo. · model: opus
- [ ] **P6-03** Blame engine via `gix` blame (fallback `git blame --porcelain --incremental`), ignore-whitespace option, annotate previous revision. Tests on renamed file fixture. · model: opus
- [ ] **P6-04** Blame UI: gutter columns (author, date, short hash) configurable, colour by author or age, hover popup with commit summary, actions: show diff, show history, copy revision, select in Log. · model: opus
- [ ] **P6-05** File history engine: `git log --follow` for file/dir, all-branches toggle, compare revisions, open at revision (blob read), revert to revision, affected paths. Tests. · model: opus
- [ ] **P6-06** File history UI: history panel from Changes/Log context menu; actions: compare, open at revision, revert, annotate, cherry-pick, create patch. · model: opus
- [ ] **P6-07** Undo v3 complete: every destructive op has a safety point; Undo menu with last 10 labelled entries; toasts standardised. · model: opus
- [ ] **P6-08** Issues (H10): list and search issues for GitHub/GitLab, create issue, "Create branch from issue" naming template; link commits mentioning `#123` in details pane. · model: sonnet
- [ ] **P6-09** Remotes management dialog; Update Project (all repos, merge/rebase, autostash/shelve); Sync-branches-across-repos setting (basic). · model: sonnet
- [ ] **P6-10** Git console panel: executed commands with args, duration, exit code, stderr; copy command. · model: sonnet
- [ ] **P6-11** Warnings: detached HEAD, CRLF, large files, protected-branch rebase; one-click fixes. · model: sonnet

## Quality

- [ ] **P6-12** Accessibility pass: full keyboard traversal, focus rings, ARIA labels on custom controls, canvas graph exposes an accessible row list, high-contrast theme, `prefers-reduced-motion`. Checklist in `docs/a11y.md`. · model: sonnet
- [ ] **P6-13** Performance pass against Linux kernel and Chromium clones: cold start, status, diff, log; fix regressions to meet §1 budgets; memory profiling (< 150 MB mid-size repo); `core.fsmonitor`/`untrackedCache` opt-in prompt for large repos. · model: best
- [ ] **P6-14** Windows spawn audit: count `git.exe` launches per view; batch or cache to hit budgets. · model: best
- [ ] **P6-25** Low-end laptop pass: run every P0 flow on the reference low-end machine (8 GB, 2 cores, SATA/HDD, 1366×768) on Windows 10 and Ubuntu; record against §4 budgets; fix until all are within 2×. Recruit at least 10 beta testers on low-end hardware. Large-repo mode itself is P6-13; this task verifies it on real low-end hardware. · model: best
- [ ] **P6-15** WebView compatibility sweep: visual regression screenshots on WKWebView, WebView2, WebKitGTK; fix CSS differences; scrollbars, fonts. · model: sonnet
- [ ] **P6-16** Crash reporting (opt-in, Sentry EU region) with client-side scrubbing of paths, file contents, messages; opt-in telemetry (weekly active ping only). Privacy copy reviewed. · model: sonnet
- [ ] **P6-17** Signed builds: Apple Developer ID + notarization; Azure Trusted Signing (or EV fallback) for Windows; GPG for Linux; secrets in CI; universal macOS binary; Windows x64 + arm64; Linux x64 + arm64 AppImage/deb/rpm. · model: sonnet
- [ ] **P6-18** Auto-updater: Tauri updater plugin, signed `latest.json` per channel (beta/stable), in-app "Update available" flow, release notes display. · model: sonnet
- [ ] **P6-19** Onboarding polish: < 60 s to first diff; import recent repos from GitHub Desktop / Fork configs (optional). · model: sonnet
- [ ] **P6-20** i18n scaffolding: all strings via `t()`, English catalogue extracted, pseudo-locale test for layout overflow. · model: sonnet
- [ ] **P6-21** Decision: open-source `git-engine` and `credential-helper` under MIT? Write ADR; if yes, split repo/licensing headers. · model: sonnet

## Beta

- [ ] **P6-22** Beta programme: Discord server, private GitHub issues repo, weekly beta builds on the `beta` channel, feedback form, crash triage routine. · model: sonnet
- [ ] **P6-23** Recruit 100–150 testers from waitlist, VS Code/Cursor communities, ex-IntelliJ users, 5–10 friendly teams on GitHub + GitLab. · model: sonnet
- [ ] **P6-24** Weekly triage: top 10 issues fixed per week; beta changelog posted; newsletter #6–#11. · model: sonnet

## Exit criteria

- Crash-free sessions ≥ 99.5 % across beta builds.
- §1 metrics met on kernel and Chromium repos on all 3 OSes.
- Appendix A items marked 1.0 in A2, A4, A5, A12 complete; G11, G12, G13, G15, H10 verified.
- Signed, notarized, auto-updating builds on all platforms.
- a11y checklist complete; high-contrast theme shipped.
