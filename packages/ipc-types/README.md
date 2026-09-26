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
import { commands, events } from '@shootgit/ipc-types';

const answer = await commands.ping(); // "pong"

// Commands that can fail resolve to a result instead of throwing:
const opened = await commands.openRepo('/work/repo');
if (opened.status === 'error') console.warn(opened.error.kind, opened.error.message);

// Typed events; `listen` resolves to the function that stops listening.
const unlisten = await events.repoChanged.listen(({ payload }) => {
  console.log(payload.repo_id, payload.kinds);
});
```

In the app, only the stores in `packages/ui/src/stores/` call these.
