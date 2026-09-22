# Phase 8 — Monetization (deferred; starts when trigger is met)

Spec: §8 Licensing & billing (full plan), §3 L1–L6.
Trigger: ≥ 5,000 active installs and clear demand (team requests, self-hosted enterprise asks). Announce 60 days ahead. Individual local Git use stays free.

## Tasks (to be expanded when triggered)

- [ ] **P8-01** ADR: which features are paid (team features, integration depth, priority support) vs free; grandfathering rules. · model: opus
- [ ] **P8-02** `licensing` crate: token schema (§8), Ed25519 verify, run rule, 14-day TTL + 30-day grace, activation (3 machines), trial. Tests for every branch of the run rule. · model: opus
- [ ] **P8-03** License server (Axum): activate/refresh/deactivate, MoR webhooks (idempotent), subscription state machine, `fallback_until` vesting, team seats, magic-link account page. · model: opus
- [ ] **P8-04** MoR: Paddle vs Dodo sandbox trial; products Monthly $5.99, Annual $49, Team $79/seat/yr; PPP pricing; VAT ID; invoice ≥ 10 seats. · model: opus
- [ ] **P8-05** In-app screens: trial banner, activate, account, fallback notice. · model: opus
- [ ] **P8-06** Full §8 test matrix in sandbox. · model: opus
- [ ] **P8-07** Pricing page with the exact wording in §8; 60-day announcement; launch paid Team plan. · model: opus

## Exit criteria

- First paid Team subscriptions; zero regression for free users; every §8 test-matrix row passes.
