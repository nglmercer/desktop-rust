import { spawn, type SpawnOptionsWithoutStdio } from 'child_process'

/** Execute one traditional hook using Git's POSIX shell fallback and stderr routing. */
export function spawnTraditionalHook(
  path: string,
  args: readonly string[],
  options: SpawnOptionsWithoutStdio
) {
  const script =
    'if [ ! -f "$1" ] || [ ! -x "$1" ]; then exit 0; fi; exec "$@" 1>&2'
  return spawn(
    '/bin/sh',
    ['-c', script, 'desktop-hook', path, ...args],
    options
  )
}
