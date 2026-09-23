import type { ResolvedConfig } from 'vite';
import solid from 'vite-plugin-solid';
import { describe, expect, it } from 'vitest';

import { solidPluginOptions } from './vite.config';

// vite-plugin-solid's hot-reload wrapper imports the virtual module id
// `/@solid-refresh`. Vitest's Node runner turns that into
// `file:///@solid-refresh`, which Windows rejects as a filename while POSIX
// accepts it, so an injected wrapper kills the whole suite on Windows only.
// These tests pin both halves of the fix on every OS: the test run must not
// get the wrapper, and the dev server must keep it.

const COMPONENT_SOURCE = 'export function Hello() {\n  return <p>hello</p>;\n}\n';
// A bare name: nothing here may depend on how an OS spells a path.
const COMPONENT_ID = 'Hello.tsx';
/**
 * What the wrapper adds to a module. The plugin emits the bare specifier and
 * its own `resolve.alias` (`/^solid-refresh$/`) rewrites it to the virtual id
 * `/@solid-refresh` at resolve time, which is the id Windows then chokes on.
 */
const REFRESH_IMPORT = 'from "solid-refresh"';

/** A plugin hook is either the function itself or `{ handler }`. */
function handlerOf<T>(hook: T | { handler: T } | undefined, name: string): T {
  if (hook === undefined) {
    throw new Error(`vite-plugin-solid has no ${name} hook any more`);
  }
  if (typeof hook === 'function') {
    return hook;
  }
  if (typeof hook === 'object' && hook !== null && 'handler' in hook) {
    return hook.handler;
  }
  throw new Error(`vite-plugin-solid's ${name} hook has an unexpected shape`);
}

/**
 * Runs the plugin's own transform over one component, the way Vite would in
 * `serve` mode, and returns the generated code.
 */
async function transformComponent(mode: string): Promise<string> {
  const plugin = solid(solidPluginOptions(mode));

  // `configResolved` is where the plugin decides whether to inject the
  // hot-reload wrapper; it reads only `command` and `mode`, so a stub with
  // those two fields drives it.
  const configResolved = handlerOf(plugin.configResolved, 'configResolved');
  await configResolved.call(
    // Neither hook touches its plugin context, so empty stubs are enough; the
    // casts keep them visibly stubs.
    {} as unknown as ThisParameterType<typeof configResolved>,
    { command: 'serve', mode } as unknown as ResolvedConfig,
  );

  const transform = handlerOf(plugin.transform, 'transform');
  // The transform reads `this.environment` to detect SSR; leaving it undefined
  // is the client case, the only one this app has.
  const result: unknown = await transform.call(
    { environment: undefined } as unknown as ThisParameterType<typeof transform>,
    COMPONENT_SOURCE,
    COMPONENT_ID,
  );

  if (typeof result === 'string') {
    return result;
  }
  if (typeof result === 'object' && result !== null && 'code' in result) {
    const { code } = result as { code: unknown };
    if (typeof code === 'string') {
      return code;
    }
  }
  throw new Error('vite-plugin-solid returned no code for a .tsx module');
}

describe('solid plugin options', () => {
  it('runs this suite in Vite mode "test", which is what turns the wrapper off', () => {
    // If Vitest ever stops using mode 'test', solidPluginOptions would hand the
    // suite a hot-reloading plugin again and Windows would break. Fail here
    // instead, on every OS.
    expect(import.meta.env.MODE).toBe('test');
  });

  it('does not inject the solid-refresh runtime in test mode', async () => {
    expect(await transformComponent('test')).not.toContain(REFRESH_IMPORT);
  });

  it('still injects the solid-refresh runtime for the dev server', async () => {
    // `pnpm tauri dev` must keep hot module reloading.
    expect(await transformComponent('development')).toContain(REFRESH_IMPORT);
  });
});
