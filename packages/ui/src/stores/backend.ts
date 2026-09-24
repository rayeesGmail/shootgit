import { commands } from '@shootgit/ipc-types';
import { type Accessor, createSignal } from 'solid-js';

/**
 * Whether the Rust side of the app is answering.
 *
 * `checking` is the state before the first answer; the UI must render in it
 * rather than wait (SPEC §4 Low-resource operation, rule 9).
 */
export type BackendStatus = 'checking' | 'connected' | 'unavailable';

/** What `ping` answers when the IPC bridge is healthy (`src-tauri/src/commands`). */
const PONG = 'pong';

/** The shape of the generated `commands.ping` binding, so tests can stand in for it. */
export type Ping = () => Promise<string>;

/**
 * Asks the Rust side once whether it is there, and exposes the answer as a
 * Solid accessor.
 *
 * `ping` is a parameter only so tests can supply their own; production code
 * calls it with no arguments and gets the generated binding, which is the only
 * way the frontend is allowed to reach Rust (SPEC §4).
 *
 * A rejected call is a normal outcome, not a crash: a browser tab with no
 * Tauri host, or a backend that died, both land on `unavailable`.
 */
export function createBackendStatus(ping: Ping = () => commands.ping()): Accessor<BackendStatus> {
  const [status, setStatus] = createSignal<BackendStatus>('checking');

  // Fired once, at creation: the status is a fact about the bridge, not a
  // derived value, so reading it must never issue another round-trip.
  void ping().then(
    (answer) => setStatus(answer === PONG ? 'connected' : 'unavailable'),
    () => setStatus('unavailable'),
  );

  return status;
}
