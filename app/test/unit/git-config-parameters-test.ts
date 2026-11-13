import { spawnSync } from 'node:child_process'
import { describe, expect, it } from 'vitest'
import { quoteGitConfigParameter } from '../../src/lib/hooks/git-config-parameters'

describe('Git environment configuration parameters', () => {
  it.each([
    "/tmp/a path/it's hooks",
    "C:\\Users\\O'Brien\\hooks",
    "path' 'desktop.injected=true",
    'path\nwith newline',
  ])('passes the hook path literally to Git: %s', path => {
    const parameters = `'desktop.inherited=kept' ${quoteGitConfigParameter(`core.hooksPath=${path}`)}`
    const run = (key: string) => spawnSync('git', ['config', '--get', key], {
      encoding: 'utf8',
      env: {
        ...process.env,
        GIT_CONFIG_COUNT: '0',
        GIT_CONFIG_NOSYSTEM: '1',
        GIT_CONFIG_GLOBAL: process.platform === 'win32' ? 'NUL' : '/dev/null',
        GIT_CONFIG_PARAMETERS: parameters,
      },
    })

    const hookPath = run('core.hooksPath')
    expect(hookPath.status, hookPath.stderr).toBe(0)
    expect(hookPath.stdout).toBe(`${path}\n`)
    expect(run('desktop.inherited').stdout).toBe('kept\n')
    expect(run('desktop.injected').status).toBe(1)
  })
})
