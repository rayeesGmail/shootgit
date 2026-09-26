import { type JSX, Show } from 'solid-js';

export interface NoticeProps {
  tone: 'error' | 'warning';
  message: string;
  /** A command the user can run to fix it; the app never runs it itself. */
  fix: string | null;
  onDismiss?: () => void;
}

/**
 * A message about something the user may need to act on, with the command
 * that fixes it when there is one (the `safe.directory` exception, the
 * inotify watch limit). The command is selectable so it can be copied.
 *
 * Strings are hardcoded English until `t()` lands (P6-20).
 */
export function Notice(props: NoticeProps): JSX.Element {
  return (
    <div class={`notice notice--${props.tone}`} role={props.tone === 'error' ? 'alert' : 'status'}>
      <p class="notice__message">{props.message}</p>
      <Show when={props.fix}>
        {(fix) => (
          <p class="notice__fix">
            Run this in a terminal to fix it: <code class="notice__command">{fix()}</code>
          </p>
        )}
      </Show>
      <Show when={props.onDismiss}>
        {(dismiss) => (
          <button type="button" class="notice__dismiss" onClick={() => dismiss()()}>
            Dismiss
          </button>
        )}
      </Show>
    </div>
  );
}
