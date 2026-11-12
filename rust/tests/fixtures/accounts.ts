/** Shared offline account and repository fixtures for migrated integration tests. */
import assert from 'node:assert/strict'
import { spawnSync, type SpawnSyncReturns } from 'node:child_process'
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'

export const ALICE = 'synthetic-assigned-alice-private'
export const BOB = 'synthetic-assigned-bob-private'
export const ENV = 'synthetic-effective-environment-private'
export const SECRET = 'synthetic-account-diagnostic-private'
export const NAMED = {
  mode: 'assigned',
  host: 'github.com',
  login: 'AliceCase',
}
export const OTHER = { ...NAMED, login: 'BobCase' }
export const NONE = { mode: 'none' }
export const EFFECTIVE = { mode: 'effective' }
export const BINARY = path.resolve('rust/target/debug/desktop-rs')
export type Json = Record<string, unknown>

export function object(value: unknown): Json {
  assert.ok(
    typeof value === 'object' && value !== null && !Array.isArray(value),
    'Expected object'
  )
  return Object.fromEntries(Object.entries(value))
}
export function list(value: unknown): ReadonlyArray<unknown> {
  assert.ok(Array.isArray(value), 'Expected array')
  return value
}
export function string(value: unknown): string {
  assert.equal(typeof value, 'string', 'Expected string')
  return String(value)
}
export function clean(value: unknown): void {
  const text = typeof value === 'string' ? value : JSON.stringify(value)
  assert.ok(
    ![ALICE, BOB, ENV, SECRET].some(secret => text.includes(secret)),
    'Output contains fixture credential'
  )
}
export function privateFile(file: string, contents: string | Buffer): void {
  writeFileSync(file, contents, { mode: 0o600 })
}

export class AccountRepo {
  constructor(
    readonly root: string,
    private readonly suite: Accounts
  ) {}
  git(...args: string[]): Buffer {
    const result = spawnSync(this.suite.realGit, ['-C', this.root, ...args], {
      env: this.suite.env,
      encoding: 'buffer',
      timeout: 20_000,
    })
    assert.ifError(result.error)
    assert.equal(result.status, 0, result.stderr?.toString())
    return result.stdout
  }
  write(name: string, content: string | Buffer): void {
    writeFileSync(path.join(this.root, name), content)
  }
  at(root: string): AccountRepo {
    return new AccountRepo(root, this.suite)
  }
}

export class Accounts {
  readonly root = mkdtempSync(path.join(tmpdir(), 'desktop-account-tests-'))
  readonly env: NodeJS.ProcessEnv
  readonly realGit: string
  readonly registry = path.join(this.root, 'accounts', 'repositories.json')
  readonly scenario = path.join(this.root, 'scenario.json')
  readonly log = path.join(this.root, 'gh-calls.jsonl')
  readonly started = path.join(this.root, 'gh-started')
  readonly release = path.join(this.root, 'gh-release')
  data: Json = {}

  constructor() {
    const bin = path.join(this.root, 'bin')
    mkdirSync(bin)
    const found = spawnSync('sh', ['-c', 'command -v git'], {
      encoding: 'utf8',
    })
    assert.equal(found.status, 0, found.stderr)
    this.realGit = found.stdout.trim()
    writeFileSync(
      path.join(bin, 'gh'),
      '#!/bin/sh\nexec "$ACCOUNT_NODE" "$ACCOUNT_GH_FIXTURE" "$@"\n',
      { mode: 0o700 }
    )
    writeFileSync(
      path.join(bin, 'git'),
      `#!/bin/sh
verify() {
  while [ "$#" -gt 0 ]; do
    case "$1" in -C|-c|--git-dir|--work-tree|--namespace) shift 2;; -*) shift;; *) command="$1"; shift; break;; esac
  done
  case "\${command-}" in fetch|push|pull|clone) return 97;; esac
  if [ "\${command-}" = ls-remote ]; then case " $* " in *' --get-url '*) ;; *) return 97;; esac; fi
  if [ "\${command-}" = remote ]; then case " $* " in *' update '*|*' prune '*|*' -f '*|*' --fetch '*) return 97;; esac; fi
}
verify "$@" || exit $?
exec "$ACCOUNT_REAL_GIT" "$@"
`,
      { mode: 0o700 }
    )
    this.env = {
      ...process.env,
      HOME: this.root,
      PATH: `${bin}${path.delimiter}${process.env.PATH ?? ''}`,
      GIT_CONFIG_NOSYSTEM: '1',
      GIT_CONFIG_GLOBAL: path.join(this.root, 'global-config'),
      GIT_TERMINAL_PROMPT: '0',
      ACCOUNT_NODE: process.execPath,
      ACCOUNT_GH_FIXTURE: path.resolve('rust/tests/fixtures/account-gh.ts'),
      ACCOUNT_REAL_GIT: this.realGit,
      CONTEXT_SCENARIO: this.scenario,
      CONTEXT_GH_LOG: this.log,
      CONTEXT_GH_STARTED: this.started,
      CONTEXT_GH_RELEASE: this.release,
      GH_TOKEN: ENV,
      GITHUB_TOKEN: BOB,
      GH_ENTERPRISE_TOKEN: ENV,
      GITHUB_ENTERPRISE_TOKEN: BOB,
      GH_REPO: 'foreign.invalid/wrong/repo',
      GH_HOST: 'foreign.invalid',
      GH_DEBUG: 'api',
      DEBUG: '1',
    }
    for (const key of [
      'GIT_DIR',
      'GIT_WORK_TREE',
      'GIT_INDEX_FILE',
      'GIT_CONFIG_COUNT',
      'GIT_CONFIG_PARAMETERS',
    ])
      delete this.env[key]
    this.configure()
  }
  configure(options: Json = {}): void {
    const host = typeof options.host === 'string' ? options.host : 'github.com'
    const base = {
      id: 2001,
      full_name: 'fork/repo',
      name: 'repo',
      html_url: `https://${host}/fork/repo`,
      owner: { login: 'fork' },
      private: true,
      fork: false,
      parent: null,
    }
    this.data = {
      host,
      active: 'BobCase',
      visible: ['AliceCase', 'BobCase'],
      signed_out: [],
      users: {
        AliceCase: { id: 101, login: 'alicecase', token: ALICE },
        BobCase: { id: 202, login: 'bobcase', token: BOB },
      },
      environment: { id: 303, login: 'environment-user', token: ENV },
      repositories: {
        alicecase: {
          ...base,
          fork: true,
          parent: {
            full_name: 'parent/repo',
            html_url: `https://${host}/parent/repo`,
          },
        },
        bobcase: structuredClone(base),
        'environment-user': structuredClone(base),
      },
      secret: SECRET,
      ...options,
    }
    for (const file of [this.started, this.release])
      rmSync(file, { force: true })
    this.save()
  }
  save(): void {
    privateFile(this.scenario, JSON.stringify(this.data))
  }
  update(options: Json): void {
    Object.assign(this.data, options)
    this.save()
  }
  calls(): ReadonlyArray<Json> {
    return existsSync(this.log)
      ? readFileSync(this.log, 'utf8')
          .trim()
          .split('\n')
          .map(line => object(JSON.parse(line)))
      : []
  }
  clear(): void {
    rmSync(this.log, { force: true })
  }
  seed(name: string, save = true): AccountRepo {
    const root = path.join(this.root, name)
    const init = spawnSync(this.realGit, ['init', '-q', '-b', 'main', root], {
      env: this.env,
      encoding: 'utf8',
    })
    assert.equal(init.status, 0, init.stderr)
    const repo = new AccountRepo(root, this)
    repo.git('config', 'user.name', 'Account Test')
    repo.git('config', 'user.email', 'account@example.invalid')
    repo.git('config', 'commit.gpgsign', 'false')
    repo.write('file', 'base\n')
    repo.git('add', '--all')
    repo.git('commit', '-qm', 'checkpoint')
    for (const branch of ['fork-main', 'parent-main', 'feature'])
      repo.git('branch', branch)
    const oid = repo.git('rev-parse', 'HEAD').toString().trim()
    const host = string(this.data.host)
    for (const [remote, owner, branch] of [
      ['origin', 'fork', 'fork-main'],
      ['upstream', 'parent', 'parent-main'],
    ]) {
      repo.git('remote', 'add', remote, `https://${host}/${owner}/repo.git`)
      repo.git('update-ref', `refs/remotes/${remote}/${branch}`, oid)
      repo.git(
        'symbolic-ref',
        `refs/remotes/${remote}/HEAD`,
        `refs/remotes/${remote}/${branch}`
      )
      repo.git('branch', '--set-upstream-to', `${remote}/${branch}`, branch)
    }
    if (save) this.cli(['repositories', 'add', repo.root])
    return repo
  }
  execute(
    args: ReadonlyArray<string>,
    options: {
      readonly payload?: string
      readonly fail?: boolean
      readonly config?: string
    } = {}
  ): SpawnSyncReturns<string> {
    const result = spawnSync(
      BINARY,
      ['--config', options.config ?? this.registry, ...args],
      {
        env: this.env,
        encoding: 'utf8',
        input: options.payload,
        timeout: 30_000,
      }
    )
    assert.ifError(result.error)
    clean(result.stdout)
    clean(result.stderr)
    assert.equal(
      result.status === 0,
      options.fail !== true,
      result.stderr || result.stdout
    )
    return result
  }
  cli(
    args: ReadonlyArray<string>,
    options: {
      readonly payload?: string
      readonly fail?: boolean
      readonly config?: string
    } = {}
  ): Json {
    const result = this.execute(args, options)
    return options.fail
      ? { stderr: result.stderr, stdout: result.stdout }
      : object(JSON.parse(result.stdout))
  }
  rpc(
    repo: AccountRepo,
    method: string,
    params: Json = {},
    options: { readonly fail?: boolean; readonly config?: string } = {}
  ): Json {
    return this.cli(['-C', repo.root, 'repo', '-'], {
      ...options,
      payload: JSON.stringify({ method, ...params }),
    })
  }
  account(repo: AccountRepo): Json {
    return this.rpc(repo, 'repository_account')
  }
  catalog(repo: AccountRepo): Json {
    return this.rpc(repo, 'repository_account_catalog')
  }
  preview(repo: AccountRepo, target: Json = NAMED): Json {
    return this.rpc(repo, 'repository_account_preview', { target })
  }
  assign(repo: AccountRepo, target: Json = NAMED): Json {
    const plan = this.preview(repo, target)
    assert.equal(plan.can_apply, true)
    return this.rpc(repo, 'set_repository_account', {
      target,
      basis: plan.basis,
    })
  }
  github(
    repo: AccountRepo,
    action: Json = { type: 'issues' },
    repository: string | null = null,
    fail = false
  ): Json {
    return this.rpc(
      repo,
      'github',
      { request: { repository, action } },
      { fail }
    )
  }
  record(repo: AccountRepo): Json {
    const data = object(JSON.parse(readFileSync(this.registry, 'utf8')))
    const record = list(data.records)
      .map(object)
      .find(value => value.path === repo.root)
    assert.ok(record, 'Missing saved repository record')
    return record
  }
  evidence(repo: AccountRepo, includeConfig = true): unknown {
    const common = repo
      .git('rev-parse', '--path-format=absolute', '--git-common-dir')
      .toString()
      .trim()
    const index = repo
      .git('rev-parse', '--path-format=absolute', '--git-path', 'index')
      .toString()
      .trim()
    const loose = path.join(repo.root, 'loose')
    return {
      refs: repo
        .git('for-each-ref', '--format=%(refname) %(objectname)')
        .toString('base64'),
      head: repo.git('symbolic-ref', '-q', 'HEAD').toString('base64'),
      index: readFileSync(index).toString('base64'),
      config: includeConfig
        ? readFileSync(path.join(common, 'config')).toString('base64')
        : null,
      name: repo.git('config', 'user.name').toString('base64'),
      email: repo.git('config', 'user.email').toString('base64'),
      file: readFileSync(path.join(repo.root, 'file')).toString('base64'),
      loose: existsSync(loose) ? readFileSync(loose).toString('base64') : null,
    }
  }
  dispose(): void {
    rmSync(this.root, { recursive: true, force: true })
  }
}
