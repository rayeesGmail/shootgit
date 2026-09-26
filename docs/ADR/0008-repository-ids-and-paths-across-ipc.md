# 0008 — Repository ids and paths across IPC

- Status: proposed
- Date: 2026-09-26
- Spec: refines §4 (IPC contract), §5 (Core models: `RepoInfo.id`)

## Context

P0-12 is the first task whose commands carry repository state to the
frontend: `open_repo`, `get_status` and the `repo-changed { repo_id, kinds }`
event. Two things the spec names but does not define had to be decided.

**How an open repository is addressed.** §5 gives `RepoInfo` an `id` and §4
gives `repo-changed` a `repo_id`, but neither says what an id is, how long it
lives, or whether it is stable across launches.

**How paths cross IPC.** The status models hold `PathBuf`s with git's raw
bytes (P0-08). On Linux a file name need not be valid UTF-8. serde refuses to
serialise such a path, so one bad file name would fail the whole
`get_status` reply. JSON strings, and so `settings.json`, cannot hold one
either (P0-11): recording such a repository as recent would make every later
settings save fail.

## Options considered

Ids:

1. **A process-local number minted per repository handle.** Every
   `git_engine::repo::Repo` gets the next `u32` when it is created; clones
   share it. The app keeps one handle per working tree. Small, cheap and
   meaningless outside the process, so nothing can persist it by mistake. It
   is a plain TypeScript `number`. A `u64` would not be: specta refuses to
   export it because JavaScript numbers cannot hold one exactly.
2. **The canonical path.** Stable across launches, but long, and it inherits
   the non-UTF-8 problem below. A repository that moves would get a new id
   anyway.
3. **A hash of the path.** Stable, but opaque, and it needs a collision
   story for no benefit the UI uses today.

Paths:

1. **Lossy strings.** Each invalid sequence becomes U+FFFD. The TypeScript
   type stays `string`. Such a path can be shown but cannot be sent back to
   name the file.
2. **Byte arrays, or a `string | { lossy, bytes }` union.** Lossless, but
   every path in the UI becomes a union to unpack, and Windows paths (UTF-16,
   possibly with unpaired surrogates) need a second encoding. That is a lot
   of cost to protect display-only data.

## Decision

Ids: option 1. `RepoId(u32)` lives in `git_engine::repo` and is minted by
`Repo::new` and `open_repo`. `status` fills `RepoInfo.id` from the handle it
ran on. The app's registry (`src-tauri/src/repos.rs`) keeps one `RepoActor`
for the one open repository (§4 rule 6). Opening that repository again
returns its existing id. Opening another closes it, so if it is opened later
it gets a new id. Ids are never persisted: across launches a repository is
known by its path.

Paths: option 1. The status models serialise paths with
`to_string_lossy` (`#[serde(serialize_with = ..)]` with
`#[specta(type = String)]`). Settings record only UTF-8 paths:
`Settings::record_recent_repo` skips any other path, so a repository at such a
path opens but is not remembered. A git path in settings is always UTF-8,
because it can only come from a JSON string.

The codec attributes made specta split every status type into
`X_Serialize | X_Deserialize` twins. The bindings builder therefore runs specta
in unified mode (`disable_serde_phases`). That mode still rejects a type it
cannot represent, and a codec field without an explicit specta type.

## Consequences

- The frontend addresses repositories by a number and shows paths as
  strings, with no unions to unpack.
- A non-UTF-8 file name shows with U+FFFD. Nothing in Phase 0 sends a path
  back to Rust. When staging arrives (P1-04, P1-05), an entry must be
  addressable losslessly: either by the entry's own raw bytes (option 2,
  added then) or by an index into the last status. That task decides which.
- `open_repo` takes the path from the dialog as a JSON string, so a
  repository whose own path is not UTF-8 cannot be opened from the UI yet.
  This is rare, and it shares the fix above.
- Ids change on every launch. Anything persisted per repository (changelists,
  shelves, the forge cache) is keyed by path or lives in `.git/<app>/`, as §4
  already says.
- The decision needs a row in the §12 Decisions log (Claude Doc, then
  re-export; `SPEC.md` is never hand-edited).
