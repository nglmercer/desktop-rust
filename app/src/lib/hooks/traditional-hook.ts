import {
  spawn,
  type ChildProcess,
  type ISpawnOptions,
} from '../../web/child_process'

/** Execute one traditional hook using Git's POSIX shell fallback and stderr routing. */
export function spawnTraditionalHook(
  path: string,
  args: readonly string[],
  options: Omit<ISpawnOptions, 'stdio'>
): ChildProcess & {
  stdin: NonNullable<ChildProcess['stdin']>
  stdout: NonNullable<ChildProcess['stdout']>
  stderr: NonNullable<ChildProcess['stderr']>
} {
  const script =
    'if [ ! -f "$1" ] || [ ! -x "$1" ]; then exit 0; fi; exec "$@" 1>&2'
  return spawn(
    '/bin/sh',
    ['-c', script, 'desktop-hook', path, ...args],
    options
  ) as ChildProcess & {
    stdin: NonNullable<ChildProcess['stdin']>
    stdout: NonNullable<ChildProcess['stdout']>
    stderr: NonNullable<ChildProcess['stderr']>
  }
}
