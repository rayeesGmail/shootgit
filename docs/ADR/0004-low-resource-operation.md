# 0004 — Low-resource operation as a Phase 0 constraint

- Status: accepted
- Date: 2026-09-22
- Spec: refines §1, §3 (N9), §4, §6

## Context
Many users run 8 GB laptops with an IDE, a browser and containers already open, often on 2 cores and a slow disk. The app has to stay responsive and small alongside them, not only on a developer workstation.

## Options considered
1. Optimise in Phase 6 — cheaper up front; but memory layout, threading and lazy loading are architectural and expensive to retrofit once every view and engine path assumes unbounded resources.
2. Budgets as Phase 0 constraints with a constrained-VM CI gate — every task is built against the low-end budgets from the start and regressions fail CI.

## Decision
Option 2. The low-end laptop is a hard target from Phase 0: a single runtime sized to the machine, one shared `GitCommand` concurrency limiter with priority lanes, cancellable reads, and a CI job that runs the smoke flow inside a 4 GB / 2 vCPU cgroup and fails on regression (P0-17, P0-18).

## Consequences
Every list is virtualised, every read is cancellable, and all git spawns and async work in our code go through the shared limiter and the single runtime; threads that libraries manage internally are allowed. Eager blame and background indexing are ruled out. Later tasks carry explicit memory and idle-CPU tests (P1-24, P2-24, P4-26), and Phase 6 verifies on real low-end hardware (P6-25).
