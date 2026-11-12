// @vitest-environment node
import {
  spawn,
  spawnSync,
  type ChildProcessWithoutNullStreams,
} from 'node:child_process'
import { existsSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { afterEach, describe, expect, it } from 'vitest'
import {
  ALICE,
  BINARY,
  EFFECTIVE,
  ENV,
  NONE,
  OTHER,
  clean,
  list,
  object,
  string,
  type AccountRepo,
  type Json,
} from './fixtures/accounts'
import { AccountBridge } from './fixtures/account-bridge'
import {
  ACTION,
  Creation,
  type CreationService,
  PR_URL,
} from './fixtures/creation'

const enabled = process.env.DESKTOP_TEST_PR_CREATE === '1'
let suite: Creation | undefined
afterEach(async () => {
  await suite?.close()
  suite = undefined
})

function graphqlCalls(s: Creation): ReadonlyArray<Json> {
  return s
    .calls()
    .filter(row => row.kind === 'api' && list(row.args).includes('graphql'))
}
function assertGraphqlArgs(row: Json): void {
  const args = list(row.args).map(string)
  expect(args[0]).toBe('api')
  const options: Record<string, string> = {}
  const positional: string[] = []
  for (let i = 1; i < args.length; i++) {
    if (args[i].startsWith('-')) {
      expect(['--method', '--hostname', '--header', '--input']).toContain(
        args[i]
      )
      expect(options[args[i]]).toBeUndefined()
      options[args[i]] = args[++i]
    } else positional.push(args[i])
  }
  expect(positional).toEqual(['graphql'])
  expect(options).toEqual({
    '--method': 'POST',
    '--hostname': 'github.com',
    '--header': 'Accept: application/vnd.github+json',
    '--input': '-',
  })
}
function assertPrivateTransport(
  s: Creation,
  service: CreationService,
  actor = 'alicecase',
  assigned = true
): void {
  const rows = s.calls()
  const received = service.records()
  clean(rows)
  clean(received)
  expect(
    rows.every(row => row.argv_clean === true && row.stdin_clean === true)
  ).toBe(true)
  expect(
    s
      .gitCalls()
      .every(
        row => row.argv_clean === true && row.alice_environment_clean === true
      )
  ).toBe(true)
  if (assigned) {
    expect(
      rows.some(row => list(row.args).slice(0, 2).join(':') === 'pr:create')
    ).toBe(false)
    const api = rows.filter(row => row.kind === 'api')
    expect(api.length).toBeGreaterThan(0)
    expect(
      api.every(
        row =>
          row.actor === actor &&
          row.pinned === true &&
          row.debug_cleared === true &&
          row.defaults_cleared === true
      )
    ).toBe(true)
    expect(received.every(row => row.actor === actor)).toBe(true)
  }
  expect(service.errors()).toBe('')
}

function openPull(
  options: {
    readonly owner?: string
    readonly base?: string
    readonly head?: string
    readonly cross?: boolean
    readonly number?: number
  } = {}
): Json {
  return {
    url: `https://github.com/fork/repo/pull/${options.number ?? 7}`,
    state: 'OPEN',
    baseRefName: options.base ?? ACTION.base,
    headRefName: options.head ?? ACTION.head.split(':').at(-1),
    isCrossRepository: options.cross ?? true,
    headRepositoryOwner: { login: options.owner ?? 'fork' },
  }
}
function failedCreate(
  s: Creation,
  repo: AccountRepo,
  action: Json = ACTION
): {
  readonly status: number | null
  readonly stdout: string
  readonly stderr: string
} {
  return s.execute(['-C', repo.root, 'repo', '-'], {
    payload: JSON.stringify({ method: 'github', request: { action } }),
    fail: true,
  })
}

async function waitFor(
  predicate: () => boolean,
  label: string,
  timeout = 12_000
): Promise<void> {
  const deadline = Date.now() + timeout
  while (!predicate() && Date.now() < deadline)
    await new Promise(resolve => setTimeout(resolve, 20))
  expect(predicate(), label).toBe(true)
}
interface IPending {
  readonly child: ChildProcessWithoutNullStreams
  readonly finished: Promise<number | null>
  readonly output: () => { readonly stdout: string; readonly stderr: string }
}
async function pendingCreate(
  s: Creation,
  service: CreationService,
  repo: AccountRepo,
  basis: unknown,
  stage: string
): Promise<IPending> {
  for (const file of [service.started, service.release])
    rmSync(file, { force: true })
  service.update({ gate: stage })
  const child = spawn(
    BINARY,
    ['--config', s.registry, '-C', repo.root, 'repo', '-'],
    { env: s.env, stdio: 'pipe' }
  )
  let stdout = ''
  let stderr = ''
  child.stdout.setEncoding('utf8').on('data', chunk => {
    stdout += chunk
  })
  child.stderr.setEncoding('utf8').on('data', chunk => {
    stderr += chunk
  })
  const finished = new Promise<number | null>((resolve, reject) => {
    child.once('error', reject)
    child.once('close', resolve)
  })
  child.stdin.on('error', () => {})
  child.stdin.end(
    JSON.stringify({
      method: 'github',
      request: { action: ACTION, account_basis: basis },
    })
  )
  try {
    await waitFor(
      () => existsSync(service.started),
      `PR ${stage} did not reach the local TLS gate`
    )
  } catch (error) {
    child.kill('SIGKILL')
    await finished
    throw error
  }
  return { child, finished, output: () => ({ stdout, stderr }) }
}
async function finishPending(
  pending: IPending,
  timeout = 15_000
): Promise<number | null> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      pending.finished,
      new Promise<never>((_, reject) => {
        timer = setTimeout(
          () => reject(new Error('Pending PR request timed out')),
          timeout
        )
      }),
    ])
  } finally {
    if (timer !== undefined) clearTimeout(timer)
  }
}
async function stopPending(
  pending: IPending,
  service: CreationService
): Promise<void> {
  writeFileSync(service.release, 'release')
  if (pending.child.exitCode === null && pending.child.signalCode === null) {
    pending.child.kill('SIGKILL')
    await finishPending(pending)
  }
}
function descendants(parent: number): ReadonlyMap<number, string> {
  const result = spawnSync('ps', ['-A', '-o', 'pid=,ppid=,comm='], {
    encoding: 'utf8',
    timeout: 5_000,
  })
  expect(result.status).toBe(0)
  const rows = result.stdout
    .trim()
    .split('\n')
    .map(line => line.trim().split(/\s+/, 3))
  const found = new Map<number, string>()
  let parents = new Set([parent])
  while (parents.size > 0) {
    const next = new Set<number>()
    for (const [pid, ppid, command] of rows) {
      const id = Number(pid)
      if (parents.has(Number(ppid)) && !found.has(id)) {
        found.set(id, command.split('/').at(-1) ?? command)
        next.add(id)
      }
    }
    parents = next
  }
  return found
}
function alive(pid: number): boolean {
  try {
    process.kill(pid, 0)
  } catch {
    return false
  }
  const status = `/proc/${pid}/status`
  try {
    return (
      !existsSync(status) || !readFileSync(status, 'utf8').includes('State:\tZ')
    )
  } catch {
    return false
  }
}

describe.skipIf(!enabled || process.platform === 'win32')(
  'assigned PR creation against local TLS',
  () => {
    it('uses real gh with pinned credentials and preserves literal GraphQL input', async () => {
      const home = process.env.HOME
      const s = new Creation()
      suite = s
      const service = await s.start()
      const f = s.seedUnborn('assigned-api-create')
      const before = s.evidence(f)
      const stored = readFileSync(s.registry)
      for (const action of [
        {
          ...ACTION,
          title: 'Ordinary PR',
          body: 'ordinary body',
          base: 'main',
          head: 'fork:main',
          draft: false,
        },
        ACTION,
        {
          ...ACTION,
          title: 'Normalized unqualified head',
          base: 'main',
          head: ':feature',
        },
      ]) {
        s.clear()
        const result = s.create(f, action)
        expect(result).toEqual({
          code: 0,
          data: null,
          stdout: `${PR_URL}\n`,
          stderr: '',
          hint: null,
        })
        const calls = graphqlCalls(s)
        expect(calls).toHaveLength(2)
        for (const row of calls) {
          assertGraphqlArgs(row)
          expect(row.native_api).toBe(true)
        }
        const [lookup, mutation] = calls.map(row =>
          object(JSON.parse(string(row.body)))
        )
        expect(string(lookup.query)).toContain(
          'DesktopPullRequestCreateContext'
        )
        expect(lookup.variables).toEqual({
          owner: 'fork',
          name: 'repo',
          head: action.head.split(':').at(-1),
          after: null,
        })
        expect(string(mutation.query)).toContain('PullRequestCreate')
        expect(string(mutation.query)).toContain('createPullRequest')
        expect(mutation.variables).toEqual({
          input: {
            repositoryId: 'R_fixture',
            title: action.title,
            body: action.body,
            baseRefName: action.base,
            headRefName: action.head.replace(/^:/, ''),
            draft: action.draft,
            maintainerCanModify: true,
          },
        })
        expect(service.mutations()).toHaveLength(1)
        expect(
          service.records().filter(row => row.stage === 'user').length
        ).toBeGreaterThanOrEqual(2)
        assertPrivateTransport(s, service)
        expect(readFileSync(s.registry)).toEqual(stored)
        expect(s.evidence(f)).toEqual(before)
      }
      expect(s.data.active).toBe('BobCase')
      expect(s.env.GH_TOKEN).toBe(ENV)
      expect(process.env.HOME).toBe(home)
    }, 180_000)
    it('checks duplicate pull requests, bounded pagination and repository identity', async () => {
      const s = new Creation()
      suite = s
      const service = await s.start()
      const f = s.seedUnborn('create-context-proof')
      const before = s.evidence(f)
      const stored = readFileSync(s.registry)
      const real = s.env.CREATE_REAL_GH
      delete s.env.CREATE_REAL_GH
      try {
        for (const head of ['main', ':main']) {
          s.clear()
          s.create(f, { ...ACTION, base: 'main', head }, undefined, true)
          expect(graphqlCalls(s)).toHaveLength(0)
          expect(service.mutations()).toHaveLength(0)
        }
        s.clear()
        service.update({ pages: [[openPull()]] })
        s.create(f, ACTION, undefined, true)
        expect(service.mutations()).toHaveLength(0)
        s.clear()
        service.update({
          pages: [
            [
              openPull({ base: 'another-base' }),
              openPull({ head: 'another-head' }),
              openPull({ owner: 'other' }),
            ],
          ],
        })
        expect(s.create(f).code).toBe(0)
        expect(service.mutations()).toHaveLength(1)
        s.clear()
        service.update({
          pages: [
            Array.from({ length: 100 }, (_, number) =>
              openPull({ head: 'not-the-head', number: number + 100 })
            ),
            [openPull()],
          ],
        })
        s.create(f, ACTION, undefined, true)
        expect(service.mutations()).toHaveLength(0)
        const lookups = service.records().filter(row => row.stage === 'lookup')
        expect(lookups).toHaveLength(2)
        expect(object(object(lookups[1].payload).variables).after).toBe(
          'cursor-1'
        )
        for (const node of [
          null,
          {
            id: '',
            nameWithOwner: 'fork/repo',
            url: 'https://github.com/fork/repo',
          },
          {
            id: 'R_wrong',
            nameWithOwner: 'other/repo',
            url: 'https://github.com/other/repo',
          },
        ]) {
          s.clear()
          service.update({ node })
          s.create(f, ACTION, undefined, true)
          expect(service.mutations()).toHaveLength(0)
        }
        for (const page_override of [
          {
            nodes: Array.from({ length: 101 }, () =>
              openPull({ head: 'different' })
            ),
            pageInfo: { hasNextPage: false, endCursor: null },
          },
          { nodes: [], pageInfo: { hasNextPage: true, endCursor: null } },
          {
            nodes: [{ ...openPull(), headRepositoryOwner: null }],
            pageInfo: { hasNextPage: false, endCursor: null },
          },
        ]) {
          s.clear()
          service.update({ page_override })
          s.create(f, ACTION, undefined, true)
          expect(service.mutations()).toHaveLength(0)
        }
        s.clear()
        service.update({
          pages: Array.from({ length: 17 }, () =>
            Array.from({ length: 100 }, (_, number) =>
              openPull({ head: 'different', number: number + 100 })
            )
          ),
        })
        s.create(f, ACTION, undefined, true)
        expect(
          service.records().filter(row => row.stage === 'lookup')
        ).toHaveLength(16)
        expect(service.mutations()).toHaveLength(0)
      } finally {
        if (real !== undefined) s.env.CREATE_REAL_GH = real
      }
      expect(readFileSync(s.registry)).toEqual(stored)
      expect(s.evidence(f)).toEqual(before)
      assertPrivateTransport(s, service)
    }, 180_000)

    it('redacts API failures and flags ambiguous mutation results', async () => {
      const s = new Creation()
      suite = s
      const service = await s.start()
      const f = s.seedUnborn('create-api-errors')
      const before = s.evidence(f)
      const stored = readFileSync(s.registry)
      const real = s.env.CREATE_REAL_GH
      delete s.env.CREATE_REAL_GH
      try {
        for (const [stage, failure] of [
          ['lookup', 'graphql'],
          ['lookup', 'malformed'],
          ['lookup', 'http'],
          ['mutation', 'graphql'],
          ['mutation', 'malformed'],
          ['mutation', 'http'],
        ] as const) {
          s.clear()
          service.update({ failures: { [stage]: failure } })
          const result = failedCreate(s, f)
          clean(result.stdout)
          clean(result.stderr)
          expect(service.mutations()).toHaveLength(stage === 'mutation' ? 1 : 0)
          assertPrivateTransport(s, service)
        }
        for (const stage of ['lookup', 'mutation']) {
          s.clear()
          s.update({ api_exits: { [stage]: 7 } })
          const result = failedCreate(s, f)
          expect(result.status).toBe(7)
          expect(object(JSON.parse(result.stdout)).code).toBe(7)
          expect(service.mutations()).toHaveLength(stage === 'mutation' ? 1 : 0)
          s.update({ api_exits: {} })
        }
        for (const url of [
          'https://foreign.invalid/fork/repo/pull/7',
          'http://github.com/fork/repo/pull/7',
          'https://github.com/other/repo/pull/7',
          'https://github.com/fork/repo/pull/0',
          `https://github.com/fork/repo/pull/7?trace=${ALICE}`,
          null,
        ]) {
          s.clear()
          service.update({ url })
          const value = object(JSON.parse(failedCreate(s, f).stdout))
          expect(value).toMatchObject({ code: 1, stdout: '', data: null })
          expect(string(value.hint).toLowerCase()).toContain('inspect')
          expect(string(value.hint).toLowerCase()).toContain('retry')
          expect(service.mutations()).toHaveLength(1)
          assertPrivateTransport(s, service)
        }
      } finally {
        if (real !== undefined) s.env.CREATE_REAL_GH = real
      }
      expect(readFileSync(s.registry)).toEqual(stored)
      expect(s.evidence(f)).toEqual(before)
    }, 180_000)

    it('fences actor, remote and account changes before a remote mutation', async () => {
      const s = new Creation()
      suite = s
      const service = await s.start()
      for (const change of ['actor', 'source', 'account']) {
        const f = s.seedUnborn(`create-prewrite-${change}`)
        const basis = s.account(f).basis
        s.clear()
        const real = s.env.CREATE_REAL_GH
        delete s.env.CREATE_REAL_GH
        const pending = await pendingCreate(s, service, f, basis, 'lookup')
        try {
          if (change === 'actor') {
            const users = object(structuredClone(s.data.users))
            users.AliceCase = { ...object(users.AliceCase), id: 999 }
            s.update({ users })
          } else if (change === 'source')
            f.git('remote', 'set-url', 'origin', 'https://github.com/fork/repo')
          else s.assign(f, OTHER)
          const before = s.evidence(f)
          const stored = readFileSync(s.registry)
          writeFileSync(service.release, 'release')
          expect(await finishPending(pending, 35_000)).not.toBe(0)
          const output = pending.output()
          clean(output.stdout)
          clean(output.stderr)
          expect(service.mutations()).toHaveLength(0)
          expect(readFileSync(s.registry)).toEqual(stored)
          expect(s.evidence(f)).toEqual(before)
        } finally {
          await stopPending(pending, service)
          if (real !== undefined) s.env.CREATE_REAL_GH = real
        }
        expect(
          s
            .gitCalls()
            .every(
              row =>
                row.argv_clean === true && row.alice_environment_clean === true
            )
        ).toBe(true)
        expect(
          s
            .calls()
            .some(row => list(row.args).slice(0, 2).join(':') === 'pr:create')
        ).toBe(false)
        expect(service.errors()).toBe('')
      }
    }, 180_000)

    it('honors saved account modes and queued account basis', async () => {
      const s = new Creation()
      suite = s
      const service = await s.start()
      const f = s.seedUnborn('create-saved-modes')
      const assignment = s.account(f).assignment
      let before = s.evidence(f)
      let stored = readFileSync(s.registry)
      s.update({ signed_out: ['AliceCase'] })
      s.clear()
      s.create(f, ACTION, undefined, true)
      expect(s.account(f).assignment).toEqual(assignment)
      expect(readFileSync(s.registry)).toEqual(stored)
      expect(service.mutations()).toHaveLength(0)
      expect(graphqlCalls(s)).toHaveLength(0)
      expect(s.evidence(f)).toEqual(before)
      s.update({ signed_out: [] })
      const old = s.account(f).basis
      s.assign(f, OTHER)
      stored = readFileSync(s.registry)
      s.clear()
      s.create(f, ACTION, old, true)
      expect(s.calls()).toHaveLength(0)
      expect(service.records()).toHaveLength(0)
      expect(readFileSync(s.registry)).toEqual(stored)
      s.assign(f, NONE)
      before = s.evidence(f)
      stored = readFileSync(s.registry)
      s.clear()
      s.create(f, ACTION, undefined, true)
      expect(s.calls()).toHaveLength(0)
      expect(service.records()).toHaveLength(0)
      expect(readFileSync(s.registry)).toEqual(stored)
      expect(s.evidence(f)).toEqual(before)
      s.assign(f, EFFECTIVE)
      before = s.evidence(f)
      stored = readFileSync(s.registry)
      s.clear()
      expect(s.create(f)).toEqual({
        code: 0,
        data: null,
        stdout: `${PR_URL}\n`,
        stderr: '',
        hint: null,
      })
      const typed = s.calls().filter(row => row.kind === 'typed')
      expect(typed).toHaveLength(1)
      expect(typed[0]).toMatchObject({
        actor: 'environment-user',
        body: ACTION.body,
      })
      expect(list(typed[0].args).slice(0, 2)).toEqual(['pr', 'create'])
      expect(list(typed[0].args)).toContain('--body-file')
      expect(service.records()).toHaveLength(0)
      expect(s.gitCalls().some(row => row.command === 'remote')).toBe(true)
      assertPrivateTransport(s, service, 'alicecase', false)
      expect(readFileSync(s.registry)).toEqual(stored)
      expect(s.evidence(f)).toEqual(before)
      expect(s.env.GH_TOKEN).toBe(ENV)
    }, 180_000)

    it('preserves remote-write ambiguity and cleans up CLI and WebSocket cancellation', async () => {
      const s = new Creation()
      suite = s
      const service = await s.start()
      for (const stage of ['lookup', 'mutation']) {
        const f = s.seedUnborn(`create-cli-cancel-${stage}`)
        const basis = s.account(f).basis
        const before = s.evidence(f)
        const stored = readFileSync(s.registry)
        s.clear()
        const pending = await pendingCreate(s, service, f, basis, stage)
        try {
          const children = descendants(pending.child.pid ?? 0)
          expect([...children.values()]).toContain('gh')
          expect(service.mutations()).toHaveLength(stage === 'mutation' ? 1 : 0)
          pending.child.kill('SIGINT')
          expect(await finishPending(pending)).toBe(130)
          const output = pending.output()
          clean(output.stdout)
          clean(output.stderr)
          expect(output.stderr.toLowerCase()).toContain('cancelled')
          if (stage === 'mutation') {
            expect(output.stderr.toLowerCase()).toContain('inspect github')
            expect(output.stderr.toLowerCase()).toContain(
              'remote mutation may have completed'
            )
          } else
            expect(output.stderr.toLowerCase()).not.toContain('inspect github')
          await waitFor(
            () => [...children.keys()].every(pid => !alive(pid)),
            'PR cancellation left a gh child running'
          )
        } finally {
          await stopPending(pending, service)
        }
        expect(service.mutations()).toHaveLength(stage === 'mutation' ? 1 : 0)
        expect(readFileSync(s.registry)).toEqual(stored)
        expect(s.evidence(f)).toEqual(before)
        assertPrivateTransport(s, service)
      }
      const f = s.seedUnborn('create-active-ws-cancel')
      const basis = s.account(f).basis
      s.clear()
      const bridge = await AccountBridge.start(s, f)
      try {
        const before = s.evidence(f)
        const stored = readFileSync(s.registry)
        service.update({ gate: 'mutation' })
        bridge.send(7001, 'github', {
          request: { account_basis: basis, action: ACTION },
        })
        try {
          await waitFor(
            () => existsSync(service.started),
            'active WebSocket mutation did not reach the TLS response gate'
          )
          expect(service.mutations()).toHaveLength(1)
          bridge.send(7002, 'cancel', { request_id: 7001 })
          const replies = new Map<number, Json>()
          while (replies.size < 2) {
            const reply = await bridge.receive()
            if (typeof reply.id === 'number') replies.set(reply.id, reply)
          }
          expect(object(replies.get(7002)?.result).status).toBe('requested')
          const cancelled = replies.get(7001)
          expect(cancelled?.error_code).toBe('cancelled')
          expect(string(cancelled?.error).toLowerCase()).toContain(
            'inspect github'
          )
          expect(string(cancelled?.error).toLowerCase()).toContain(
            'remote mutation may have completed'
          )
        } finally {
          writeFileSync(service.release, 'release')
        }
        expect(
          object((await bridge.call('repository_account')).assignment).login
        ).toBe('AliceCase')
        expect(service.mutations()).toHaveLength(1)
        expect(readFileSync(s.registry)).toEqual(stored)
        expect(s.evidence(f)).toEqual(before)
        assertPrivateTransport(s, service)
      } finally {
        await bridge.close()
      }
    }, 180_000)
  }
)
