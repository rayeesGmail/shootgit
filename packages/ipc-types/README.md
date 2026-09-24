# `@shootgit/ipc-types`

`bindings.ts` is **generated** from the Rust command signatures in
`src-tauri/src/commands/` and **committed**. Do not edit it by hand.

```sh
pnpm gen:types   # rewrites bindings.ts from Rust
```

Why it is committed, when the rest of the tree's generated files are not:
the frontend has to typecheck and run its tests without a Rust toolchain, and
a change to the IPC surface has to be visible in a pull request diff
(ADR 0003). CI regenerates the file and fails the build if the committed copy
differs, so a forgotten `pnpm gen:types` cannot reach `main`.

The single source of the IPC surface is `app_lib::ipc::builder`
(`src-tauri/src/ipc.rs`): that one value is what Tauri dispatches through and
what the generator reads.

Use it from the UI as:

```ts
import { commands } from '@shootgit/ipc-types';

const answer = await commands.ping(); // "pong"
```
