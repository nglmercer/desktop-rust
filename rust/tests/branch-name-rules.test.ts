// @vitest-environment node
import { spawn, spawnSync, type ChildProcess } from 'node:child_process'
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'

const enabled = process.env.DESKTOP_TEST_BRANCH_RULES === '1'
const binary = path.resolve('rust/target/debug/desktop-rs')
const TOKEN = 'synthetic-policy-session-private'
const SECOND = 'synthetic-policy-other-account-private'
const ENV_TOKEN = 'synthetic-policy-env-private'
const SECRET = 'policy-private-diagnostic-sentinel'
const PREFIX = '/repos/fork/repo'
const secrets = [TOKEN, SECOND, ENV_TOKEN, SECRET]

type Json = Record<string, unknown>
function object(value: unknown): Json {
  if (typeof value !== 'object' || value === null || Array.isArray(value))
    throw new Error('Expected object')
  return Object.fromEntries(Object.entries(value))
}
function list(value: unknown): ReadonlyArray<unknown> {
  if (!Array.isArray(value)) throw new Error('Expected array')
  return value
}
function string(value: unknown): string {
  if (typeof value !== 'string') throw new Error('Expected string')
  return value
}
function clean(value: unknown): void {
  const text = typeof value === 'string' ? value : JSON.stringify(value)
  expect(secrets.some(secret => text.includes(secret))).toBe(false)
}
function pattern(id = 11, text = 'approved/', operator = 'starts_with'): Json {
  return {
    type: 'branch_name_pattern',
    ruleset_id: id,
    parameters: { operator, pattern: text, negate: false },
  }
}

class Repo {
  constructor(
    readonly root: string,
    private readonly suite: RuleSuite
  ) {}
  git(...args: string[]): Buffer {
    const result = spawnSync(this.suite.realGit, ['-C', this.root, ...args], {
      env: this.suite.env,
      encoding: 'buffer',
      timeout: 20_000,
    })
    expect(result.error).toBeUndefined()
    expect(result.status, result.stderr?.toString()).toBe(0)
    return result.stdout
  }
  write(name: string, content: string | Buffer): void {
    writeFileSync(path.join(this.root, name), content)
  }
}

class RuleSuite {
  readonly root = mkdtempSync(path.join(tmpdir(), 'desktop-branch-rules-'))
  readonly env: NodeJS.ProcessEnv
  readonly realGit: string
  readonly scenario = path.join(this.root, 'scenario.json')
  readonly log = path.join(this.root, 'gh-calls.jsonl')
  readonly started = path.join(this.root, 'gh-started')
  readonly release = path.join(this.root, 'gh-release')
  readonly token = path.join(this.root, 'gh-token')
  data: Json = {}

  constructor() {
    const bin = path.join(this.root, 'bin')
    mkdirSync(bin)
    const found = spawnSync('sh', ['-c', 'command -v git'], {
      encoding: 'utf8',
    })
    expect(found.status).toBe(0)
    this.realGit = found.stdout.trim()
    writeFileSync(
      path.join(bin, 'gh'),
      '#!/bin/sh\nexec "$RULE_NODE" "$RULE_GH_FIXTURE" "$@"\n',
      { mode: 0o700 }
    )
    writeFileSync(
      path.join(bin, 'git'),
      `#!/bin/sh
verify() {
  while [ "$#" -gt 0 ]; do
    case "$1" in
      -C|-c|--git-dir|--work-tree|--namespace) shift 2;;
      -*) shift;;
      *) command="$1"; shift; break;;
    esac
  done
  case "\${command-}" in fetch|push|pull|clone) return 97;; esac
  if [ "\${command-}" = ls-remote ]; then
    case " $* " in *' --get-url '*) ;; *) return 97;; esac
  fi
  if [ "\${command-}" = remote ]; then
    case " $* " in *' update '*|*' prune '*|*' -f '*|*' --fetch '*) return 97;; esac
  fi
}
verify "$@" || exit $?
exec "$RULE_REAL_GIT" "$@"
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
      RULE_NODE: process.execPath,
      RULE_GH_FIXTURE: path.resolve('rust/tests/fixtures/branch-rules-gh.ts'),
      RULE_REAL_GIT: this.realGit,
      RULE_TOKEN_FILE: this.token,
      CONTEXT_SCENARIO: this.scenario,
      CONTEXT_GH_LOG: this.log,
      CONTEXT_GH_STARTED: this.started,
      CONTEXT_GH_RELEASE: this.release,
      GH_REPO: 'unrelated.invalid/wrong/repository',
      GH_HOST: 'unrelated.invalid',
      GH_DEBUG: 'api',
      DEBUG: '1',
    }
    for (const key of [
      'GH_TOKEN',
      'GITHUB_TOKEN',
      'GH_ENTERPRISE_TOKEN',
      'GITHUB_ENTERPRISE_TOKEN',
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
    const fork = options.fork !== false
    this.data = {
      host,
      prefix: PREFIX,
      metadata: {
        nameWithOwner: 'fork/repo',
        url: `https://${host}/fork/repo`,
        isFork: fork,
        parent: fork
          ? { nameWithOwner: 'parent/repo', url: `https://${host}/parent/repo` }
          : null,
      },
      user: { id: 1001, login: 'fork', plan: { name: 'pro' } },
      repository: {
        id: 2001,
        full_name: 'fork/repo',
        name: 'repo',
        html_url: `https://${host}/fork/repo`,
        owner: { login: 'fork' },
        private: false,
      },
      rules: [],
      rulesets: {},
      expected_token: TOKEN,
      other_token: SECOND,
      secret: SECRET,
      ...options,
    }
    for (const file of [
      this.started,
      this.release,
      `${this.started}-user-count`,
    ])
      rmSync(file, { force: true })
    writeFileSync(this.token, TOKEN, { mode: 0o600 })
    this.save()
  }
  save(): void {
    writeFileSync(this.scenario, JSON.stringify(this.data), { mode: 0o600 })
    chmodSync(this.scenario, 0o600)
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
  apis(): ReadonlyArray<Json> {
    return this.calls().filter(call => call.kind === 'api')
  }
  seed(name: string, verified = true): Repo {
    const root = path.join(this.root, name)
    const init = spawnSync(this.realGit, ['init', '-q', '-b', 'main', root], {
      env: this.env,
      encoding: 'utf8',
    })
    expect(init.status, init.stderr).toBe(0)
    const repo = new Repo(root, this)
    repo.git('config', 'user.name', 'Rules Test')
    repo.git('config', 'user.email', 'rules@example.invalid')
    repo.git('config', 'commit.gpgsign', 'false')
    repo.write('file', 'base\n')
    repo.git('add', '--all')
    repo.git('commit', '-qm', 'checkpoint')
    for (const branch of ['fork-main', 'parent-main', 'feature'])
      repo.git('branch', branch)
    const host = string(this.data.host)
    const oid = repo.git('rev-parse', 'HEAD').toString().trim()
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
    if (verified)
      expect(this.rpc(repo, 'refresh_repository_context').status).toBe(
        'verified'
      )
    return repo
  }
  execute(
    repo: Repo,
    args: ReadonlyArray<string>,
    input?: string,
    fail = false
  ): { stdout: string; stderr: string; status: number | null } {
    const result = spawnSync(binary, ['-C', repo.root, ...args], {
      env: this.env,
      encoding: 'utf8',
      input,
      timeout: 30_000,
    })
    expect(result.error).toBeUndefined()
    clean(result.stdout)
    clean(result.stderr)
    expect(result.status === 0, result.stderr).toBe(!fail)
    return result
  }
  rpc(repo: Repo, method: string, options: Json = {}, fail = false): Json {
    const result = this.execute(
      repo,
      ['repo', '-'],
      JSON.stringify({ method, ...options }),
      fail
    )
    return fail ? { stderr: result.stderr } : object(JSON.parse(result.stdout))
  }
  rules(repo: Repo, name = 'approved/topic', fail = false): Json {
    return this.rpc(repo, 'branch_name_rules', { name }, fail)
  }
  evidence(repo: Repo): unknown {
    const common = repo
      .git('rev-parse', '--path-format=absolute', '--git-common-dir')
      .toString()
      .trim()
    const index = repo
      .git('rev-parse', '--path-format=absolute', '--git-path', 'index')
      .toString()
      .trim()
    const receipts: Record<string, string> = {}
    const visit = (directory: string, prefix: string): void => {
      if (!existsSync(directory)) return
      for (const item of readdirSync(directory, { withFileTypes: true })) {
        const name = path.join(directory, item.name)
        const key = path.join(prefix, item.name)
        if (item.isDirectory()) visit(name, key)
        else if (item.isFile())
          receipts[key] = readFileSync(name).toString('base64')
      }
    }
    for (const name of [
      'desktop-checkout',
      'desktop-branch-renames',
      'desktop-branch-deletions',
    ])
      visit(path.join(common, name), name)
    const loose = path.join(repo.root, 'loose')
    return {
      refs: repo
        .git('for-each-ref', '--format=%(refname) %(objectname)')
        .toString('base64'),
      head: repo.git('symbolic-ref', '-q', 'HEAD').toString('base64'),
      index: readFileSync(index).toString('base64'),
      config: readFileSync(path.join(common, 'config')).toString('base64'),
      file: readFileSync(path.join(repo.root, 'file')).toString('base64'),
      loose: existsSync(loose) ? readFileSync(loose).toString('base64') : null,
      stash: repo.git('stash', 'list', '--format=%H %gs').toString('base64'),
      receipts,
    }
  }
  dispose(): void {
    rmSync(this.root, { recursive: true, force: true })
  }
}

let suite: RuleSuite | undefined
afterEach(() => {
  suite?.dispose()
  suite = undefined
})
describe.skipIf(!enabled || process.platform === 'win32')(
  'offline branch name rules',
  () => {
    it('pins fixed reads, encodes branch names, and never mutates the repository', () => {
      const s = new RuleSuite()
      suite = s
      const f = s.seed('fixed-reads')
      s.clear()
      const before = s.evidence(f)
      const name = 'feature/雪/%2F#tag{literal}'
      const view = object(
        JSON.parse(s.execute(f, ['branch-name-rules', name]).stdout)
      )
      expect(view).toMatchObject({
        status: 'checked',
        result: 'pass',
        branch: name,
        repository: { host: 'github.com', owner: 'fork', name: 'repo' },
        actor: { id: 1001, login: 'fork' },
      })
      expect(string(view.basis)).toHaveLength(64)
      expect(s.evidence(f)).toEqual(before)
      expect(s.calls().map(row => row.kind)).toEqual([
        'token',
        'api',
        'api',
        'api',
        'api',
      ])
      expect(s.apis().map(row => list(row.args).at(-1))).toEqual([
        '/user',
        PREFIX,
        `${PREFIX}/rules/branches/${encodeURIComponent(
          name
        )}?per_page=100&page=1`,
        '/user',
      ])
      for (const row of s.calls()) {
        const args = list(row.args)
        expect(row).toMatchObject({
          debug_cleared: true,
          repo_cleared: true,
          host_cleared: true,
        })
        expect(args[args.indexOf('--hostname') + 1]).toBe('github.com')
        expect(args).not.toContain('--repo')
        if (row.kind === 'api') {
          expect(args.slice(1, 3)).toEqual(['--method', 'GET'])
          expect(row.pinned).toBe(true)
        }
      }
      expect(list(s.calls()[0].args)).toContain('--no-refresh')
      expect(s.rpc(f, 'repository_context').contribution_remote).toBe(
        'upstream'
      )
      clean(s.calls())
      f.git(
        'remote',
        'set-url',
        'origin',
        `https://private:${SECRET}@github.com/fork/repo.git?token=${SECRET}`
      )
      s.rpc(f, 'refresh_repository_context')
      s.clear()
      expect(s.rules(f).result).toBe('pass')
      clean(s.calls())
      const count = s.calls().length
      for (const invalid of ['--help', 'bad name', 'bad\nname'])
        s.rules(f, invalid, true)
      s.rpc(f, 'branch_name_rules', { name: 'main', endpoint: '/user' }, true)
      expect(s.calls()).toHaveLength(count)
    }, 180_000)

    it('preserves authentication and host capability behavior', () => {
      const s = new RuleSuite()
      suite = s
      let f = s.seed('unverified', false)
      s.clear()
      expect(s.rules(f)).toMatchObject({ status: 'skipped', actor: null })
      expect(s.calls()).toHaveLength(0)
      s.configure({ host: 'github.example' })
      f = s.seed('unsupported-ghes')
      s.clear()
      expect(s.rules(f).status).toBe('skipped')
      expect(s.calls()).toHaveLength(0)
      s.configure({ host: 'tenant.ghe.com' })
      f = s.seed('supported-residency')
      s.clear()
      expect(s.rules(f).status).toBe('checked')
      expect(
        s.calls().every(row => {
          const args = list(row.args)
          return args[args.indexOf('--hostname') + 1] === 'tenant.ghe.com'
        })
      ).toBe(true)
      s.configure()
      f = s.seed('auth-failures')
      for (const [code, noAccount, status] of [
        [1, true, 'skipped'],
        [1, false, 'unavailable'],
        [4, false, 'skipped'],
        [7, true, 'unavailable'],
      ] as const) {
        s.configure({ failures: { token: { code } }, no_account: noAccount })
        s.clear()
        expect(s.rules(f).status).toBe(status)
        expect(s.apis()).toHaveLength(0)
        expect(s.calls().map(row => row.kind)).toEqual(
          code === 1 ? ['token', 'status'] : ['token']
        )
      }
      for (const token of ['', 'two\nlines', 'with space']) {
        s.configure({ token_raw: token })
        s.clear()
        expect(s.rules(f).status).toBe('unavailable')
        expect(s.apis()).toHaveLength(0)
      }
      s.configure()
      const details: Json = { ...object(s.data.repository), private: true }
      for (const plan of [null, 'free']) {
        const user = {
          id: 1001,
          login: 'fork',
          ...(plan === null ? {} : { plan: { name: plan } }),
        }
        s.configure({ user, repository: details })
        s.clear()
        expect(s.rules(f)).toMatchObject({
          status: 'skipped',
          result: 'not_checked',
        })
        expect(s.apis().map(row => list(row.args).at(-1))).toEqual([
          '/user',
          PREFIX,
        ])
      }
      s.configure({
        user: { id: 1002, login: 'collaborator', plan: { name: 'free' } },
        repository: details,
      })
      expect(s.rules(f).status).toBe('checked')
      details.owner = { login: 'Fork' }
      s.configure({
        user: { id: 1001, login: 'fork', plan: { name: 'free' } },
        repository: details,
      })
      expect(s.rules(f).status).toBe('checked')
    }, 180_000)

    it('preserves policy outcomes, bypass handling and bounded pagination', () => {
      const s = new RuleSuite()
      suite = s
      s.configure({
        rules: [pattern()],
        rulesets: { 11: { id: 11, current_user_can_bypass: 'never' } },
      })
      const f = s.seed('policy-results')
      const blocked = s.rules(f, 'blocked')
      expect(blocked).toMatchObject({ status: 'checked', result: 'blocked' })
      expect(list(blocked.failed)).toHaveLength(1)
      expect(s.rules(f).result).toBe('pass')
      s.update({
        rulesets: { 11: { id: 11, current_user_can_bypass: 'always' } },
      })
      const warning = s.rules(f, 'blocked')
      expect(warning).toMatchObject({ result: 'warning', failed: [] })
      expect(list(warning.bypassed).length).toBeGreaterThan(0)
      s.update({ rules: [{ type: 'creation', ruleset_id: 11 }] })
      expect(s.rules(f).result).toBe('pass')
      s.update({
        rulesets: {
          11: { id: 11, current_user_can_bypass: 'pull_requests_only' },
        },
      })
      expect(s.rules(f).result).toBe('blocked')
      s.update({ rules: [pattern()], rulesets: {} })
      expect(s.rules(f, 'blocked')).toMatchObject({
        status: 'unavailable',
        result: 'pass',
      })
      s.update({
        rules: [pattern(), pattern(12)],
        rulesets: { 11: { id: 11, current_user_can_bypass: 'never' } },
      })
      expect(s.rules(f, 'blocked')).toMatchObject({
        status: 'unavailable',
        result: 'blocked',
      })
      const irrelevant = { type: 'required_signatures', ruleset_id: 999 }
      s.configure({
        pages: { 1: Array(100).fill(irrelevant), 2: [pattern()] },
        rulesets: {
          11: { id: 11, current_user_can_bypass: 'never' },
          999: { id: 999, current_user_can_bypass: 'never' },
        },
      })
      s.clear()
      expect(s.rules(f, 'blocked').result).toBe('blocked')
      const pages = s
        .apis()
        .map(row => string(list(row.args).at(-1)))
        .filter(endpoint => endpoint.includes('/rules/branches/'))
      expect(pages).toHaveLength(2)
      expect(pages[0]).toMatch(/&page=1$/)
      expect(pages[1]).toMatch(/&page=2$/)
      expect(
        s
          .apis()
          .filter(row =>
            string(list(row.args).at(-1)).includes('/rulesets/999')
          )
      ).toHaveLength(1)
      s.configure({ rules: Array(100).fill(irrelevant) })
      s.clear()
      expect(s.rules(f).status).toBe('unavailable')
      const capped = s
        .apis()
        .map(row => string(list(row.args).at(-1)))
        .filter(endpoint => endpoint.includes('/rules/branches/'))
      expect(capped).toHaveLength(16)
      expect(capped.at(-1)).toMatch(/&page=16$/)
    }, 180_000)

    it('blocks branch and worktree mutations without changing staged or unstaged data', () => {
      const s = new RuleSuite()
      suite = s
      s.configure({
        rules: [pattern()],
        rulesets: { 11: { id: 11, current_user_can_bypass: 'never' } },
      })
      const f = s.seed('denied-mutations')
      f.git('switch', '-q', 'feature')
      f.write('file', 'saved on feature\n')
      const saved = string(s.rpc(f, 'stash_save', { desktop: true }).oid)
      f.git('switch', '-q', 'main')
      f.write('file', 'staged payload\n')
      f.git('add', '--', 'file')
      f.write('file', 'working payload\n')
      f.write('loose', Buffer.from('untracked\0'))
      for (const checkout of [false, true]) {
        const name = checkout ? 'blocked-checkout' : 'blocked-ref'
        const plan = s.rpc(f, 'create_branch_preview', { name, checkout })
        expect(object(plan.name_rules).result).toBe('blocked')
        const before = s.evidence(f)
        s.rpc(
          f,
          'create_branch',
          {
            name,
            checkout,
            strategy: 'StashOnCurrentBranch',
            basis: plan.basis,
          },
          true
        )
        expect(s.evidence(f)).toEqual(before)
      }
      const rename = s.rpc(f, 'rename_branch_preview', {
        name: 'feature',
        new_name: 'blocked-rename',
      })
      expect(object(rename.name_rules).result).toBe('blocked')
      const beforeRename = s.evidence(f)
      s.rpc(
        f,
        'rename_branch',
        { name: 'feature', new_name: 'blocked-rename', basis: rename.basis },
        true
      )
      expect(s.evidence(f)).toEqual(beforeRename)
      expect(f.git('show', `${saved}:file`).toString()).toBe(
        'saved on feature\n'
      )
      for (const newBranch of [true, false]) {
        const name = newBranch
          ? 'blocked-worktree-new'
          : 'blocked-worktree-dwim'
        if (!newBranch)
          f.git(
            'update-ref',
            `refs/remotes/origin/${name}`,
            f.git('rev-parse', 'HEAD').toString().trim()
          )
        const destination = path.join(s.root, name)
        const before = s.evidence(f)
        s.rpc(
          f,
          'worktree_add',
          { path: destination, branch: name, new_branch: newBranch },
          true
        )
        expect(s.evidence(f)).toEqual(before)
        expect(existsSync(destination)).toBe(false)
      }
      s.configure({ failures: { token: { code: 7 } } })
      s.clear()
      expect(
        s.rpc(f, 'rename_branch', { name: 'feature', new_name: 'feature' })
          .unchanged
      ).toBe(true)
      const destination = path.join(s.root, 'existing-worktree')
      expect(
        s.rpc(f, 'worktree_add', {
          path: destination,
          branch: 'feature',
          new_branch: false,
        }).code
      ).toBe(0)
      expect(s.calls()).toHaveLength(0)
      f.git('worktree', 'remove', destination)
    }, 180_000)

    it('rejects stale reviewed actor, repository, policy and contribution context', () => {
      const s = new RuleSuite()
      suite = s
      for (const operation of ['ref', 'checkout', 'rename']) {
        for (const change of ['actor', 'repository', 'rules']) {
          s.configure()
          const f = s.seed(`stale-${operation}-${change}`)
          f.write('file', 'dirty reviewed data\n')
          f.write('loose', 'keep loose\n')
          const name = `reviewed-${operation}-${change}`
          const plan =
            operation === 'rename'
              ? s.rpc(f, 'rename_branch_preview', {
                  name: 'feature',
                  new_name: name,
                })
              : s.rpc(f, 'create_branch_preview', {
                  name,
                  checkout: operation === 'checkout',
                })
          if (change === 'actor')
            s.update({
              user: { ...object(s.data.user), id: 1002, login: 'second-user' },
            })
          else if (change === 'repository')
            s.update({ repository: { ...object(s.data.repository), id: 2002 } })
          else
            s.update({
              rules: [pattern(11, 'reviewed')],
              rulesets: { 11: { id: 11, current_user_can_bypass: 'never' } },
            })
          const before = s.evidence(f)
          const rejected =
            operation === 'rename'
              ? s.rpc(
                  f,
                  'rename_branch',
                  { name: 'feature', new_name: name, basis: plan.basis },
                  true
                )
              : s.rpc(
                  f,
                  'create_branch',
                  {
                    name,
                    checkout: operation === 'checkout',
                    strategy: 'StashOnCurrentBranch',
                    basis: plan.basis,
                  },
                  true
                )
          expect(string(rejected.stderr).toLowerCase()).toContain('review')
          expect(s.evidence(f)).toEqual(before)
        }
      }
      s.configure()
      const f = s.seed('stale-contribution-context')
      const plan = s.rpc(f, 'create_branch_preview', {
        name: 'reviewed-context',
      })
      const context = s.rpc(f, 'repository_context')
      s.rpc(f, 'set_fork_contribution_target', {
        target: 'self',
        basis: context.basis,
      })
      const before = s.evidence(f)
      s.rpc(
        f,
        'create_branch',
        { name: 'reviewed-context', basis: plan.basis },
        true
      )
      expect(s.evidence(f)).toEqual(before)
    }, 180_000)

    it('pins one credential and rechecks the actor without exposing secrets', () => {
      const s = new RuleSuite()
      suite = s
      s.configure({ rotate_token: true })
      const f = s.seed('pinned-token')
      s.clear()
      const baseline = s.rules(f)
      expect(baseline.status).toBe('checked')
      expect(readFileSync(s.token, 'utf8')).toBe(SECOND)
      expect(s.apis().every(row => row.pinned === true)).toBe(true)
      expect(s.apis().some(row => row.store_still_original === false)).toBe(
        true
      )
      expect(s.calls().filter(row => row.kind === 'token')).toHaveLength(1)
      s.configure({ expected_token: SECOND })
      writeFileSync(s.token, SECOND)
      expect(s.rules(f).basis).toBe(baseline.basis)
      s.configure({ expected_token: ENV_TOKEN, rotate_token: true })
      Object.assign(s.env, {
        GH_TOKEN: ENV_TOKEN,
        GITHUB_TOKEN: SECOND,
        GH_ENTERPRISE_TOKEN: SECOND,
        GITHUB_ENTERPRISE_TOKEN: SECOND,
      })
      try {
        s.clear()
        expect(s.rules(f).status).toBe('checked')
        expect(s.apis().every(row => row.pinned === true)).toBe(true)
      } finally {
        for (const key of [
          'GH_TOKEN',
          'GITHUB_TOKEN',
          'GH_ENTERPRISE_TOKEN',
          'GITHUB_ENTERPRISE_TOKEN',
        ])
          delete s.env[key]
      }
      s.configure({ change_actor_on_second_user: true })
      expect(s.rules(f).status).toBe('unavailable')
      clean(s.calls())
    }, 180_000)

    it('rejects invalid API data without disclosing private diagnostics', () => {
      const s = new RuleSuite()
      suite = s
      const f = s.seed('invalid-api-data')
      for (const endpoint of [
        '/user',
        PREFIX,
        `${PREFIX}/rules/branches/approved%2Ftopic?per_page=100&page=1`,
      ]) {
        s.configure({ failures: { [endpoint]: { code: 1 } } })
        expect(s.rules(f).status).toBe('unavailable')
        s.configure({
          failures: { [endpoint]: { code: 0, raw: `{broken ${SECRET}` } },
        })
        expect(s.rules(f).status).toBe('unavailable')
      }
      s.configure({ rules: Array.from({ length: 101 }, () => ({})) })
      expect(s.rules(f).status).toBe('unavailable')
      s.configure()
      s.update({
        repository: {
          ...object(s.data.repository),
          html_url: 'https://github.com/foreign/repo',
        },
      })
      expect(s.rules(f)).toMatchObject({
        status: 'unavailable',
        result: 'blocked',
      })
      for (const [ruleset, status] of [
        [{ id: 999, current_user_can_bypass: 'always' }, 'unavailable'],
        [{ id: 11 }, 'checked'],
        [{ id: 11, current_user_can_bypass: 'unknown' }, 'checked'],
      ] as const) {
        s.configure({ rules: [pattern()], rulesets: { 11: ruleset } })
        expect(s.rules(f, 'blocked')).toMatchObject({
          status,
          result: 'blocked',
        })
      }
      clean(s.calls())
    }, 180_000)

    it('cancels or rejects a delayed request when its repository context changes', async () => {
      const s = new RuleSuite()
      suite = s
      const endpoint = `${PREFIX}/rules/branches/approved%2Frace?per_page=100&page=1`
      for (const label of ['origin-change', 'cancel']) {
        s.configure({ delay_endpoint: endpoint })
        const f = s.seed(label)
        let before = s.evidence(f)
        const child: ChildProcess = spawn(
          binary,
          ['-C', f.root, 'branch-name-rules', 'approved/race'],
          { env: s.env, stdio: ['ignore', 'pipe', 'pipe'] }
        )
        let stdout = ''
        let stderr = ''
        child.stdout?.on('data', chunk => {
          stdout += chunk.toString()
        })
        child.stderr?.on('data', chunk => {
          stderr += chunk.toString()
        })
        const exited = new Promise<number | null>((resolve, reject) => {
          child.once('error', reject)
          child.once('close', resolve)
        })
        try {
          const deadline = Date.now() + 10_000
          while (
            !existsSync(s.started) &&
            Date.now() < deadline &&
            child.exitCode === null
          )
            await new Promise(resolve => setTimeout(resolve, 20))
          expect(
            existsSync(s.started),
            'branch-rule request did not reach delayed GET'
          ).toBe(true)
          if (label === 'origin-change') {
            f.git(
              'remote',
              'set-url',
              'origin',
              'https://github.com/another/repo.git'
            )
            before = s.evidence(f)
            writeFileSync(s.release, 'release')
          } else child.kill('SIGINT')
          let timer: ReturnType<typeof setTimeout> | undefined
          const code = await Promise.race([
            exited,
            new Promise<never>((_, reject) => {
              timer = setTimeout(
                () => reject(new Error('delayed request did not exit')),
                10_000
              )
            }),
          ]).finally(() => {
            if (timer !== undefined) clearTimeout(timer)
          })
          clean(stdout)
          clean(stderr)
          expect(code).toBe(label === 'cancel' ? 130 : 1)
          expect(s.evidence(f)).toEqual(before)
        } finally {
          if (child.exitCode === null && child.signalCode === null) {
            child.kill('SIGKILL')
            await exited
          }
        }
      }
    }, 180_000)
  }
)
