// @vitest-environment node
import { spawn, type ChildProcess } from 'node:child_process'
import {
  chmod,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  writeFile,
} from 'node:fs/promises'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { createInterface } from 'node:readline'
import { describe, expect, it } from 'vitest'

const enabled = process.env.DESKTOP_TEST_BRIDGE === '1'
const require = createRequire(import.meta.url)
const binary = path.resolve('rust/target/debug/desktop-rs')
const shellQuote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`
interface IResult {
  readonly code: number | null
  readonly stdout: string
  readonly stderr: string
}
interface ICall {
  readonly args: string[]
  readonly stdin: string
}
interface IFixtureSocket {
  onmessage: ((event: { data: unknown }) => void) | null
  onopen: (() => void) | null
  onerror: (() => void) | null
  send(data: string): void
  close(): void
}
interface IFixtureDom {
  window: { WebSocket: new (url: string) => IFixtureSocket; close(): void }
}

async function run(
  command: string,
  args: string[],
  env: NodeJS.ProcessEnv,
  input = ''
): Promise<IResult> {
  return await new Promise((resolve, reject) => {
    const child = spawn(command, args, { env, stdio: 'pipe' })
    let stdout = '',
      stderr = ''
    child.stdout.setEncoding('utf8').on('data', data => {
      stdout += data
    })
    child.stderr.setEncoding('utf8').on('data', data => {
      stderr += data
    })
    child.once('error', reject)
    child.once('close', code => resolve({ code, stdout, stderr }))
    child.stdin.on('error', error => {
      if ((error as NodeJS.ErrnoException).code !== 'EPIPE') reject(error)
    })
    child.stdin.end(input)
    setTimeout(() => child.kill('SIGKILL'), 20_000).unref()
  })
}

describe.skipIf(!enabled || process.platform === 'win32')(
  'offline GitHub adapter',
  () => {
    it('preserves arguments, input, redaction, errors, and the WebSocket adapter', async () => {
      const root = await mkdtemp(path.join(tmpdir(), 'desktop-gh-adapter-'))
      let server: ChildProcess | undefined
      let dom: IFixtureDom | undefined
      try {
        const repo = path.join(root, 'repo'),
          bin = path.join(root, 'bin'),
          log = path.join(root, 'calls.jsonl')
        await mkdir(bin)
        const init = await run('git', ['init', '-q', repo], process.env)
        expect(init.code).toBe(0)
        const stub = path.join(bin, 'gh')
        await writeFile(
          stub,
          `#!/bin/sh\nexec ${shellQuote(
            process.execPath
          )} --import ${shellQuote(require.resolve('tsx'))} ${shellQuote(
            path.resolve('rust/tests/fixtures/gh.ts')
          )} "$@"\n`
        )
        await chmod(stub, 0o700)
        const env = {
          ...process.env,
          HOME: root,
          PATH: `${bin}${path.delimiter}${process.env.PATH ?? ''}`,
          GH_TEST_LOG: log,
          GIT_CONFIG_NOSYSTEM: '1',
          GIT_CONFIG_GLOBAL: path.join(root, 'gitconfig'),
        }
        const request = (
          action: object,
          repository: string | null = 'enterprise.example/owner/repo',
          mode = 'ok'
        ) =>
          run(
            binary,
            ['-C', repo, 'github', '-'],
            { ...env, GH_TEST_MODE: mode },
            JSON.stringify({ repository, action })
          )
        const last = async (): Promise<ICall> =>
          JSON.parse(
            (await readFile(log, 'utf8')).trim().split('\n').at(-1) ?? ''
          )
        const auth = await request(
          { type: 'auth_status', hostname: 'github.com' },
          null
        )
        expect(auth.code).toBe(0)
        expect(auth.stdout).not.toContain('DO-NOT-EXPOSE')
        expect(JSON.parse(auth.stdout).data.hosts['github.com'][0].login).toBe(
          'test-user'
        )
        expect((await last()).args).toContain('--no-refresh')
        expect((await last()).args).not.toContain('--show-token')
        for (const action of [
          { type: 'repository' },
          { type: 'pull_requests' },
          { type: 'pull_request', number: 7 },
          { type: 'pull_request_checks', number: 7 },
          { type: 'issues' },
          { type: 'issue', number: 7 },
          { type: 'workflow_runs' },
          { type: 'workflow_run', id: 99 },
        ]) {
          const result = await request(action)
          expect(result.code, result.stderr).toBe(0)
          expect(JSON.parse(result.stdout).data).toBeDefined()
          const args = (await last()).args
          expect(
            action.type === 'repository' ? args.slice(-1) : args.slice(-2)
          ).toEqual(
            action.type === 'repository'
              ? ['enterprise.example/owner/repo']
              : ['--repo', 'enterprise.example/owner/repo']
          )
        }
        const body =
          'First paragraph\n\nLiteral $(not-a-shell) and `text`\n' +
          'Long body\n'.repeat(4000)
        for (const action of [
          {
            type: 'create_pull_request',
            title: 'Fix $value',
            body,
            base: 'main',
            head: 'test-user:feature',
            draft: true,
          },
          { type: 'create_issue', title: 'Issue', body },
          { type: 'comment', number: 7, body },
          {
            type: 'review_pull_request',
            number: 7,
            review: 'request_changes',
            body,
          },
          {
            type: 'merge_pull_request',
            number: 7,
            strategy: 'squash',
            head_commit: 'a'.repeat(40),
          },
          { type: 'checkout_pull_request', number: 7 },
          { type: 'close_issue', number: 7 },
          { type: 'reopen_issue', number: 7 },
          { type: 'rerun_workflow', id: 99, failed_only: true },
        ]) {
          const result = await request(action)
          expect(result.code, result.stderr).toBe(0)
          expect(JSON.parse(result.stdout).code).toBe(0)
          const call = await last()
          if ('body' in action) {
            expect(call.stdin).toBe(body)
            expect(call.args).toContain('--body-file')
            expect(call.args).not.toContain(body)
          }
          if (action.type === 'create_pull_request') {
            expect(call.args).toContain('--draft')
            expect(call.args).toContain('--head')
          }
          if (action.type === 'merge_pull_request') {
            expect(call.args).toContain('--match-head-commit')
            expect(call.args).toContain('a'.repeat(40))
            expect(call.args).not.toContain('--admin')
          }
        }
        const previous = await readFile(log, 'utf8')
        for (const [action, repository] of [
          [{ type: 'pull_requests', limit: 0 }, 'owner/repo'],
          [{ type: 'pull_requests', state: 'invalid' }, 'owner/repo'],
          [{ type: 'issue', number: 0 }, 'owner/repo'],
          [
            {
              type: 'merge_pull_request',
              number: 7,
              strategy: 'squash',
              head_commit: 'HEAD',
            },
            'owner/repo',
          ],
          [{ type: 'auth_status', hostname: '--help' }, null],
          [{ type: 'repository' }, '--help'],
          [
            {
              type: 'create_pull_request',
              title: 'Fix',
              body: 'body',
              base: 'main',
              head: '--help',
            },
            'owner/repo',
          ],
        ] as const) {
          expect((await request(action, repository)).code).not.toBe(0)
        }
        expect(await readFile(log, 'utf8')).toBe(previous)
        const unauthenticated = await request(
          { type: 'issues' },
          undefined,
          'auth_required'
        )
        expect(unauthenticated.code).toBe(4)
        expect(JSON.parse(unauthenticated.stdout).hint).toContain('auth login')
        const pending = await request(
          { type: 'pull_request_checks', number: 7 },
          undefined,
          'pending'
        )
        expect(pending.code).toBe(8)
        expect(JSON.parse(pending.stdout).data[0].state).toBe('PENDING')
        const rejected = await request(
          { type: 'create_issue', title: 'Issue', body: 'x'.repeat(400000) },
          undefined,
          'reject_input'
        )
        expect(rejected.code).toBe(4)
        expect(JSON.parse(rejected.stdout).hint).toContain('auth login')
        const invalid = await request(
          { type: 'repository' },
          undefined,
          'invalid'
        )
        expect(invalid.code).not.toBe(0)
        expect(invalid.stderr).toContain('invalid JSON')
        const missing = await run(
          binary,
          [
            '-C',
            repo,
            'github',
            JSON.stringify({ action: { type: 'auth_status' } }),
          ],
          { ...env, PATH: path.join(root, 'missing-bin') }
        )
        expect(missing.code).not.toBe(0)
        expect(missing.stderr).toContain('start gh')
        server = spawn(
          binary,
          [
            '--config',
            path.join(root, 'registry.json'),
            'serve',
            '--empty',
            '--port',
            '0',
          ],
          { env, stdio: ['ignore', 'ignore', 'pipe'] }
        )
        const child = server
        const lines = createInterface({ input: child.stderr! })
        const startup = await new Promise<string>((resolve, reject) => {
          lines.once('line', resolve)
          child.once('error', reject)
          child.once('exit', code =>
            reject(new Error(`bridge exited: ${code}`))
          )
        })
        const match = /^Open (http:\/\/[^/]+)\/#token=(\S+)/.exec(startup)
        if (!match) throw new Error(`unexpected bridge startup: ${startup}`)
        const JSDOM = require('jsdom').JSDOM as new (
          html: string,
          options: { url: string }
        ) => typeof dom
        dom = new JSDOM('', { url: match[1] })
        if (!dom) throw new Error('missing DOM')
        const socket = new dom.window.WebSocket(
          match[1].replace('http:', 'ws:') + '/ws'
        )
        const messages: unknown[] = []
        socket.onmessage = event => {
          const message = JSON.parse(String(event.data))
          if (!('event' in message)) messages.push(message)
        }
        await new Promise<void>((resolve, reject) => {
          socket.onopen = () => resolve()
          socket.onerror = () => reject(new Error('socket error'))
        })
        const receive = async (): Promise<Record<string, any>> => {
          const until = Date.now() + 10_000
          while (!messages.length) {
            if (Date.now() > until) throw new Error('socket response timeout')
            await new Promise(resolve => setTimeout(resolve, 10))
          }
          return messages.shift() as Record<string, any>
        }
        socket.send(JSON.stringify({ token: match[2] }))
        expect((await receive()).ready).toBe(true)
        socket.send(
          JSON.stringify({
            id: 1,
            method: 'github',
            request: { action: { type: 'auth_status' } },
          })
        )
        expect(
          (await receive()).result.data.hosts['github.com'][0].active
        ).toBe(true)
        socket.send(
          JSON.stringify({ id: 2, method: 'open_repository', path: repo })
        )
        expect((await receive()).result.root).toBe(repo)
        socket.send(
          JSON.stringify({
            id: 3,
            method: 'github',
            request: { action: { type: 'pull_requests' } },
          })
        )
        expect((await receive()).result.data[0].name).toBe('test')
        socket.close()
      } finally {
        dom?.window.close()
        if (server && server.exitCode === null && server.signalCode === null) {
          const child = server
          const exited = new Promise<void>(resolve =>
            child.once('exit', () => resolve())
          )
          child.kill('SIGINT')
          await exited
        }
        await rm(root, { recursive: true, force: true })
      }
    }, 120_000)
  }
)
