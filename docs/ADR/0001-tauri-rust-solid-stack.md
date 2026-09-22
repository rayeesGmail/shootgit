# 0001 — Tauri 2 + Rust core + SolidJS frontend

- Status: accepted
- Date: 2026-09-19
- Spec: refines §4

## Context
Requirements: best Git support, best UX, small and fast, macOS + Windows + Linux from one codebase, free 1.0.

## Options considered
1. Electron + TypeScript — one Chromium everywhere, huge ecosystem; 150+ MB download, high RAM.
2. Kotlin + Compose Multiplatform — closest to IntelliJ's own code; ships a JVM, slow start.
3. Tauri 2 + Rust + web frontend — 10–20 MB, native performance for Git work; three WebView engines to test.
4. Native per OS — best feel; three codebases.

## Decision
Tauri 2 with a Rust core (git CLI for writes, gitoxide for reads) and SolidJS + TypeScript UI. CodeMirror 6 for diff/merge, canvas for the commit graph.

## Consequences
Must test WebKit, WebView2 and WebKitGTK; avoid bleeding-edge CSS. Rust learning curve is accepted in exchange for performance and a small binary.
