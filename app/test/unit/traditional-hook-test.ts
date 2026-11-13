// @vitest-environment node
import { spawnSync } from 'node:child_process'
import { EventEmitter } from 'node:events'
import { PassThrough } from 'node:stream'
import { mkdtempSync, mkdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { spawnTraditionalHook } from '../../src/lib/hooks/traditional-hook'
import { createHooksProxy } from '../../src/lib/hooks/hooks-proxy'
import type { TerminalOutput } from '../../src/lib/git/core'

describe.skipIf(process.platform === 'win32')('traditional Git hook execution', () => {
  let root = ''
  afterEach(() => { if (root) rmSync(root, { recursive: true, force: true }); root = '' })

  it('honors arguments, stdin, stderr routing, and shell fallback', async () => {
    root = mkdtempSync(join(tmpdir(), 'desktop-hook-exec-'))
    const hook = join(root, 'hook')
    writeFileSync(hook, 'printf "arg:%s\\n" "$1"; cat; printf error >&2\n', { mode: 0o755 })
    const child = spawnTraditionalHook(hook, ['hello world'], { cwd: root })
    const stdout: Buffer[] = []; const stderr: Buffer[] = []
    child.stdout.on('data', data => stdout.push(data))
    child.stderr.on('data', data => stderr.push(data))
    child.stdin.end('input\n')
    const code = await new Promise<number | null>((resolve, reject) => child.on('error', reject).on('close', resolve))
    expect(code).toBe(0)
    expect(Buffer.concat(stdout).toString()).toBe('')
    expect(Buffer.concat(stderr).toString()).toBe('arg:hello world\ninput\nerror')
  })

  it('ignores hooks removed or made non-executable after discovery', async () => {
    root = mkdtempSync(join(tmpdir(), 'desktop-hook-missing-'))
    const hook = join(root, 'pre-auto-gc')
    for (const exists of [false, true]) {
      if (exists) writeFileSync(hook, '#!/bin/sh\nexit 3\n', { mode: 0o644 })
      const child = spawnTraditionalHook(hook, [], { cwd: root })
      child.stdin.end()
      const code = await new Promise<number | null>((resolve, reject) => child.on('error', reject).on('close', resolve))
      expect(code).toBe(0)
    }
  })

  it('runs configured and traditional hooks once each when Git invokes the proxy', () => {
    root = mkdtempSync(join(tmpdir(), 'desktop-hook-chain-'))
    const git = (...args: string[]) => spawnSync('git', args, { cwd: root, encoding: 'utf8' })
    expect(git('init', '-q').status).toBe(0)
    const original = join(root, 'original'); const proxy = join(root, 'proxy')
    mkdirSync(original); mkdirSync(proxy)
    const traditional = join(original, 'pre-push')
    const target = join(root, 'hook-target')
    writeFileSync(target, 'printf "traditional:%s:%s:" "$1" "$2" >> events; cat >> events\n', { mode: 0o755 })
    symlinkSync(target, traditional)
    const runner = join(root, 'runner.mjs')
    const helper = pathToFileURL(resolve('app/src/lib/hooks/traditional-hook.ts')).href
    writeFileSync(runner, `import { spawnTraditionalHook } from ${JSON.stringify(helper)};\nconst child = spawnTraditionalHook(process.argv[2], process.argv.slice(3), { stdio: 'inherit' });\nchild.on('close', code => process.exit(code ?? 1));\n`)
    const loader = createRequire(import.meta.url).resolve('tsx')
    const quote = (value: string) => `'${value.replace(/'/g, "'\\''")}'`
    writeFileSync(join(proxy, 'pre-push'), `#!/bin/sh\nexec ${quote(process.execPath)} --import ${quote(loader)} ${quote(runner)} ${quote(traditional)} "$@"\n`, { mode: 0o755 })
    expect(git('config', 'hook.example.event', 'pre-push').status).toBe(0)
    expect(git('config', '--add', 'hook.example.event', 'reference-transaction').status).toBe(0)
    expect(git('config', 'hook.example.command', 'printf configured\\n >> events').status).toBe(0)
    if (git('hook', 'list', 'pre-push').status !== 0) return
    const input = join(root, 'input'); writeFileSync(input, 'refs/heads/a 123 refs/heads/b 456\n')
    const result = git('-c', `core.hooksPath=${proxy}`, 'hook', 'run', `--to-stdin=${input}`, 'pre-push', '--', 'origin', 'url')
    expect(result.status, result.stderr).toBe(0)
    const events = readFileSync(join(root, 'events'), 'utf8')
    expect(events.match(/traditional:/g)).toHaveLength(1)
    expect(events.match(/configured/g)).toHaveLength(1)
    expect(events).toContain('traditional:origin:url:refs/heads/a 123 refs/heads/b 456\n')
    const reference = git('-c', `core.hooksPath=${proxy}`, 'hook', 'run', `--to-stdin=${input}`, 'reference-transaction', '--', 'prepared')
    expect(reference.status, reference.stderr).toBe(0)
    expect(readFileSync(join(root, 'events'), 'utf8').match(/configured/g)).toHaveLength(2)
  })

  it('delivers buffered stdin and an ignored failure through the actual proxy', async () => {
    root = mkdtempSync(join(tmpdir(), 'desktop-hook-proxy-'))
    const hooks = join(root, 'hooks'); const temp = join(root, 'temp')
    mkdirSync(hooks); mkdirSync(temp)
    writeFileSync(join(hooks, 'pre-push'), '#!/bin/sh\nprintf "%s:%s:" "$1" "$2"; cat; exit 7\n', { mode: 0o755 })
    const conn = Object.assign(new EventEmitter(), {
      stdin: new PassThrough(), stderr: new PassThrough(),
      getArgs: async () => ['/proxy/pre-push', 'origin', 'url'],
      getEnv: async () => ({}), getCwd: async () => root,
      isStdinConnected: async () => true, exit: vi.fn(async () => {}),
    })
    const failure = vi.fn(async (_name: string, _output: TerminalOutput) => 'ignore' as const)
    const progress = vi.fn()
    const proxy = createHooksProxy(async () => ({ kind: 'success', env: { ...process.env, GIT_EXEC_PATH: '/unused' } }), temp, progress, failure, undefined, hooks)
    conn.stdin.end('refs/input\n')
    await proxy(conn as never)
    expect(failure).toHaveBeenCalledOnce()
    expect(Buffer.concat(failure.mock.calls[0][1] as Buffer[]).toString()).toContain('origin:url:refs/input\n')
    expect(conn.exit).toHaveBeenCalledWith(0)
    expect(progress).toHaveBeenLastCalledWith({ hookName: 'pre-push', status: 'finished' })
  })

  it('accepts a successful hook that exits without reading large stdin', async () => {
    root = mkdtempSync(join(tmpdir(), 'desktop-hook-early-'))
    const hooks = join(root, 'hooks'); const temp = join(root, 'temp')
    mkdirSync(hooks); mkdirSync(temp)
    writeFileSync(join(hooks, 'pre-push'), '#!/bin/sh\nexit 0\n', { mode: 0o755 })
    const conn = Object.assign(new EventEmitter(), {
      stdin: new PassThrough(), stderr: new PassThrough(),
      getArgs: async () => ['/proxy/pre-push', 'origin', 'url'],
      getEnv: async () => ({}), getCwd: async () => root,
      isStdinConnected: async () => true, exit: vi.fn(async () => {}),
    })
    const failure = vi.fn(async (_name: string, _output: TerminalOutput) => 'abort' as const)
    const proxy = createHooksProxy(async () => ({ kind: 'success', env: { ...process.env, GIT_EXEC_PATH: '/unused' } }), temp, undefined, failure, undefined, hooks)
    conn.stdin.end(Buffer.alloc(2 * 1024 * 1024, 65))
    await proxy(conn as never)
    expect(conn.exit).toHaveBeenCalledWith(0)
    expect(failure).not.toHaveBeenCalled()
  })
})
