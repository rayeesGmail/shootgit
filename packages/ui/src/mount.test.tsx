import { afterEach, describe, expect, it } from 'vitest';

import { mountApp } from './mount';

const ROOT_HTML = '<div id="root"></div>';

afterEach(() => {
  document.body.innerHTML = '';
});

describe('mountApp', () => {
  it('renders the app name as the window heading', () => {
    document.body.innerHTML = ROOT_HTML;

    const dispose = mountApp();

    const heading = document.querySelector('h1');
    expect(heading).not.toBeNull();
    // Literal on purpose: the placeholder product name (SPEC §12) is spelled
    // out here so renaming the app has to touch this test.
    expect(heading?.textContent).toBe('Shootgit');

    dispose();
  });

  it('removes everything it rendered when disposed', () => {
    document.body.innerHTML = ROOT_HTML;
    const root = document.getElementById('root');

    const dispose = mountApp();
    expect(root?.textContent).not.toBe('');

    dispose();
    expect(root?.textContent).toBe('');
  });

  it('fails with a readable error when the document has no #root', () => {
    document.body.innerHTML = '<main></main>';

    expect(() => mountApp()).toThrow(/no element with id "root"/);
  });
});
