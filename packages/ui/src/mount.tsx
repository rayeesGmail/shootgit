import { render } from 'solid-js/web';

import { App } from './App';

/** The element `index.html` reserves for the app. */
const ROOT_ID = 'root';

/**
 * Mounts the app into `#root` and returns Solid's dispose function.
 *
 * Looking the element up (instead of a non-null assertion) keeps a missing or
 * renamed `#root` a readable error rather than a blank window, and lets the
 * mount path be unit-tested. `doc` is a parameter only so tests can pass their
 * own document.
 */
export function mountApp(doc: Document = document): () => void {
  const root = doc.getElementById(ROOT_ID);
  if (root === null) {
    throw new Error(`Cannot mount the app: no element with id "${ROOT_ID}".`);
  }
  return render(() => <App />, root);
}
