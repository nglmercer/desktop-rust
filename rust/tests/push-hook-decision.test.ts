// @vitest-environment node
import { execFileSync, spawn, type ChildProcess } from 'node:child_process'
import { chmod, mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { createInterface } from 'node:readline'
import { JSDOM } from 'jsdom'
import { afterEach, expect, it, vi } from 'vitest'
import { BackendClient } from '../../app/src/backend/client'

let directory: string | undefined
let server: ChildProcess | undefined
let dom: JSDOM | undefined
const clients: BackendClient[] = []

afterEach(async () => {
  for (const client of clients.splice(0)) {
    client.close()
  }
  if (server && server.exitCode === null && server.signalCode === null) {
    const child = server
    await new Promise<void>(resolve => {
      const timer = setTimeout(() => child.kill('SIGKILL'), 5_000)
      child.once('exit', () => {
        clearTimeout(timer)
        resolve()
      })
      child.kill('SIGTERM')
    })
  }
  dom?.window.close()
  vi.unstubAllGlobals()
  if (directory) {
    await rm(directory, { recursive: true, force: true })
  }
})

it.skipIf(process.platform === 'win32')(
  'binds live push-hook decisions to the owning WebSocket request',
  async () => {
    directory = await mkdtemp(path.join(tmpdir(), 'desktop-push-decision-'))
    const env: NodeJS.ProcessEnv = {
      ...process.env,
      HOME: directory,
    }
    for (const key of Object.keys(env)) {
      if (key.startsWith('GIT_')) {
        delete env[key]
      }
    }
    Object.assign(env, {
      GIT_CONFIG_NOSYSTEM: '1',
      GIT_CONFIG_GLOBAL: path.join(directory, 'global.gitconfig'),
      GIT_TERMINAL_PROMPT: '0',
    })
    const git = (...args: string[]) =>
      execFileSync('git', args, {
        env,
        encoding: 'utf8',
        timeout: 20_000,
      }).trim()
    server = spawn(
      path.resolve('rust/target/debug/desktop-rs'),
      [
        '--config',
        path.join(directory, 'registry.json'),
        'serve',
        '--empty',
        '--port',
        '0',
      ],
      { env, stdio: ['ignore', 'ignore', 'pipe'] }
    )
    const lines = createInterface({ input: server.stderr! })
    const address = await new Promise<string>((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error('bridge startup timed out')),
        20_000
      )
      lines.once('line', line => {
        clearTimeout(timer)
        resolve(line)
      })
      server!.once('error', reject)
      server!.once('exit', code => reject(new Error(`bridge exited: ${code}`)))
    })
    expect(address.startsWith('Open ')).toBe(true)
    dom = new JSDOM('', { url: address.slice(5) })
    vi.stubGlobal('window', dom.window)
    vi.stubGlobal('sessionStorage', dom.window.sessionStorage)
    vi.stubGlobal('WebSocket', dom.window.WebSocket)
    vi.stubGlobal('location', dom.window.location)
    const client = new BackendClient()
    clients.push(client)
    await client.waitReady()
    const attacker = new BackendClient()
    clients.push(attacker)
    await attacker.waitReady()
    const socket = (client as any).ws.socket as WebSocket
    const otherSocket = (attacker as any).ws.socket as WebSocket
    const frame = (target: WebSocket, predicate: (data: any) => boolean) =>
      new Promise<any>((resolve, reject) => {
        const timer = setTimeout(() => {
          target.removeEventListener('message', listener)
          reject(new Error('frame timed out'))
        }, 20_000)
        const listener = (event: MessageEvent) => {
          const data = JSON.parse(event.data)
          if (!predicate(data)) {
            return
          }
          clearTimeout(timer)
          target.removeEventListener('message', listener)
          resolve(data)
        }
        target.addEventListener('message', listener)
      })
    for (const mode of ['approve', 'reject', 'cancel'] as const) {
      const repo = path.join(directory, `repo-${mode}`)
      const remote = path.join(directory, `remote-${mode}.git`)
      await mkdir(repo)
      git('init', '-q', '-b', 'main', repo)
      git('init', '--bare', '-q', remote)
      git('-C', repo, 'config', 'user.name', 'Test')
      git('-C', repo, 'config', 'user.email', 'test@example.invalid')
      await writeFile(path.join(repo, 'file'), mode)
      git('-C', repo, 'add', 'file')
      git('-C', repo, 'commit', '-qm', mode)
      git('-C', repo, 'remote', 'add', 'origin', remote)
      const hook = path.join(repo, '.git/hooks/pre-push')
      await writeFile(hook, '#!/bin/sh\necho tested-hook-failure >&2\nexit 1\n')
      await chmod(hook, 0o755)
      await client.selectBrowserRepository(repo)
      const decisionFrame = frame(
        socket,
        data => data.event === 'hook_decision'
      )
      const cancellation = new AbortController()
      let answer: ((value: 'abort' | 'ignore') => void) | undefined
      const push = client.pushRepositoryRemote(
        repo,
        {
          remote: 'origin',
          expected_url: remote,
          local_branch: 'main',
          remote_branch: null,
          tags: [],
          force_with_lease: false,
          no_verify: false,
          report_progress: false,
          require_hook_callbacks: true,
          interactive_hook_decision: true,
        },
        undefined,
        undefined,
        undefined,
        cancellation.signal,
        output => {
          expect(Buffer.from(output).toString()).toContain(
            'tested-hook-failure'
          )
          return new Promise(resolve => {
            answer = resolve
          })
        }
      )
      const decision = await decisionFrame
      expect(answer).toBeDefined()
      if (mode === 'approve') {
        const attackerReply = frame(otherSocket, data => data.id === 8000001)
        otherSocket.send(
          JSON.stringify({
            id: 8000001,
            method: 'hook_decision',
            request_id: decision.request_id,
            decision_id: decision.decision_id,
            ignore_failure: true,
          })
        )
        expect((await attackerReply).result.accepted).toBe(false)
        const wrongRequestReply = frame(socket, data => data.id === 8000002)
        socket.send(
          JSON.stringify({
            id: 8000002,
            method: 'hook_decision',
            request_id: decision.request_id + 1,
            decision_id: decision.decision_id,
            ignore_failure: true,
          })
        )
        expect((await wrongRequestReply).result.accepted).toBe(false)
        answer!('ignore')
        expect(await push).toBeUndefined()
        expect(git('-C', remote, 'rev-parse', 'refs/heads/main')).toBe(
          git('-C', repo, 'rev-parse', 'HEAD')
        )
      } else if (mode === 'reject') {
        answer!('abort')
        expect((await push)?.exitCode).not.toBe(0)
        expect(() =>
          git('-C', remote, 'rev-parse', '--verify', 'refs/heads/main')
        ).toThrow()
      } else {
        const jobFinished = frame(
          socket,
          data => data.id === decision.request_id
        )
        const finished = expect(push).rejects.toThrow()
        cancellation.abort()
        await finished
        await jobFinished
        const lateReply = frame(socket, data => data.id === 8000003)
        socket.send(
          JSON.stringify({
            id: 8000003,
            method: 'hook_decision',
            request_id: decision.request_id,
            decision_id: decision.decision_id,
            ignore_failure: true,
          })
        )
        expect((await lateReply).result.accepted).toBe(false)
        expect(() =>
          git('-C', remote, 'rev-parse', '--verify', 'refs/heads/main')
        ).toThrow()
      }
    }
  },
  120_000
)
