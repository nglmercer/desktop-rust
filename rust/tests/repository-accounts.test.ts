// @vitest-environment node
import { spawn } from 'node:child_process'
import {
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  writeFileSync,
} from 'node:fs'
import path from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { AccountBridge } from './fixtures/account-bridge'
import {
  Accounts,
  BINARY,
  EFFECTIVE,
  NAMED,
  NONE,
  OTHER,
  SECRET,
  clean,
  list,
  object,
  privateFile,
  string,
} from './fixtures/accounts'

const enabled = process.env.DESKTOP_TEST_ACCOUNTS === '1'
let suite: Accounts | undefined
afterEach(() => {
  suite?.dispose()
  suite = undefined
})

describe.skipIf(!enabled || process.platform === 'win32')(
  'offline repository accounts',
  () => {
    it('discovers accounts without mutating unsaved or saved repositories', () => {
      const s = new Accounts()
      suite = s
      const f = s.seed('unsaved-account', false)
      expect(s.account(f)).toMatchObject({
        saved: false,
        scope_id: null,
        assignment: EFFECTIVE,
      })
      expect(existsSync(s.registry)).toBe(false)
      s.rpc(f, 'repository_account_preview', { target: NAMED }, { fail: true })
      expect(existsSync(s.registry)).toBe(false)
      const catalog = s.catalog(f)
      expect(catalog).toMatchObject({
        status: 'available',
        account: { assignment: EFFECTIVE },
      })
      const accounts = list(catalog.accounts).map(object)
      expect(accounts.map(row => row.login)).toEqual(['AliceCase', 'BobCase'])
      expect(accounts[1].active).toBe(true)
      expect(existsSync(s.registry)).toBe(false)
      s.cli(['repositories', 'add', f.root])
      const before = readFileSync(s.registry)
      const evidence = s.evidence(f)
      const account = s.account(f)
      expect(account).toMatchObject({
        saved: true,
        revision: 0,
        assignment: EFFECTIVE,
      })
      s.clear()
      s.catalog(f)
      expect(readFileSync(s.registry)).toEqual(before)
      expect(s.evidence(f)).toEqual(evidence)
      expect(
        s
          .calls()
          .every(
            row =>
              row.kind === 'catalog' &&
              row.tokens_absent === true &&
              row.debug_cleared === true &&
              row.defaults_cleared === true
          )
      ).toBe(true)
      expect(object(s.rpc(f, 'snapshot').repository_account)).toMatchObject({
        scope_id: account.scope_id,
        assignment: EFFECTIVE,
      })
      s.update({ visible: ['AliceCase'] })
      expect(object(s.catalog(f).account).assignment).toEqual(EFFECTIVE)
      clean(s.calls())
    }, 180_000)

    it('applies verified account metadata atomically and preserves repository data', () => {
      const s = new Accounts()
      suite = s
      const f = s.seed('account-transitions')
      const before = s.evidence(f)
      const initial = s.account(f)
      const target = { ...NAMED, login: 'ALICECASE' }
      const plan = s.preview(f, target)
      expect(plan).toMatchObject({
        status: 'verified',
        can_apply: true,
        actor: { id: 101 },
      })
      expect(s.account(f)).toEqual(initial)
      expect(object(s.record(f).account).association).toBeNull()
      const view = s.rpc(f, 'set_repository_account', {
        target,
        basis: plan.basis,
      })
      expect(view).toMatchObject({
        assignment: { ...NAMED, id: 101 },
        revision: 1,
      })
      const record = object(s.record(f).account)
      expect(record.assignment).toEqual(view.assignment)
      expect(object(object(record.association).metadata).is_fork).toBe(true)
      clean(readFileSync(s.registry, 'utf8'))
      expect(s.rpc(f, 'repository_context')).toMatchObject({
        status: 'verified',
        repository: { is_fork: true },
        contribution_remote: 'upstream',
      })
      expect(s.evidence(f)).toEqual(before)
      const stored = readFileSync(s.registry)
      s.update({ visible: ['BobCase'] })
      const missing = s.preview(f)
      expect(missing).toMatchObject({ can_apply: false, status: 'unavailable' })
      s.rpc(
        f,
        'set_repository_account',
        { target: NAMED, basis: missing.basis },
        { fail: true }
      )
      expect(readFileSync(s.registry)).toEqual(stored)
      expect(object(s.account(f).assignment).id).toBe(101)
      s.configure()
      s.assign(f, NONE)
      expect(s.account(f).assignment).toEqual(NONE)
      expect(object(s.record(f).account).association).toBeNull()
      expect(s.rpc(f, 'repository_context').repository).toBeNull()
      s.clear()
      s.github(f, undefined, null, true)
      expect(s.calls()).toHaveLength(0)
      expect(s.evidence(f)).toEqual(before)
      s.assign(f, EFFECTIVE)
      expect(s.account(f).assignment).toEqual(EFFECTIVE)
      expect(s.rpc(f, 'repository_context').repository).toBeNull()
      s.rpc(f, 'refresh_repository_context')
      expect(s.rpc(f, 'repository_context')).toMatchObject({
        status: 'verified',
        repository: { is_fork: false },
      })
      expect(s.github(f, undefined, 'github.com/fork/repo').code).toBe(0)
      expect(
        s
          .calls()
          .filter(row => row.kind === 'typed')
          .at(-1)?.actor
      ).toBe('environment-user')
      const previous = readFileSync(s.registry)
      s.update({ api_failures: { '/repos/fork/repo': 1 } })
      const unavailable = s.preview(f)
      expect(unavailable.can_apply).toBe(false)
      s.rpc(
        f,
        'set_repository_account',
        { target: NAMED, basis: unavailable.basis },
        { fail: true }
      )
      expect(readFileSync(s.registry)).toEqual(previous)
      expect(s.evidence(f)).toEqual(before)
      s.configure()
      const g = s.seed('account-parent-add')
      g.git('remote', 'remove', 'upstream')
      const withoutConfig = s.evidence(g, false)
      s.assign(g)
      expect(g.git('remote', 'get-url', 'upstream').toString()).toBe(
        'https://github.com/parent/repo.git\n'
      )
      expect(s.evidence(g, false)).toEqual(withoutConfig)
      const h = s.seed('account-without-origin')
      h.git('remote', 'rename', 'origin', 'z-fork')
      h.git('remote', 'remove', 'upstream')
      s.assign(h)
      expect(s.rpc(h, 'repository_context')).toMatchObject({
        status: 'verified',
        remote: 'z-fork',
      })
      const saved = object(JSON.parse(readFileSync(s.registry, 'utf8')))
      const records = list(saved.records).map(object)
      const entry = records.find(row => row.path === h.root)
      expect(entry).toBeDefined()
      if (entry === undefined) throw new Error('missing account')
      const account = object(entry.account)
      const association = object(account.association)
      delete account.source_remote
      delete association.remote_name
      account.association = association
      entry.account = account
      saved.records = records
      privateFile(s.registry, JSON.stringify(saved))
      expect(s.rpc(h, 'repository_context')).toMatchObject({
        status: 'verified',
        remote: 'z-fork',
      })
      s.assign(h, NONE)
      expect(s.account(h).remote).toBe('z-fork')
      expect(s.rpc(h, 'repository_context').repository).toBeNull()
      s.assign(h, EFFECTIVE)
      s.rpc(h, 'refresh_repository_context')
      expect(s.rpc(h, 'repository_context')).toMatchObject({
        status: 'verified',
        remote: 'z-fork',
        repository: { is_fork: false },
      })
    }, 180_000)

    it('pins typed operations to the selected account and handles signed-out identities', () => {
      const s = new Accounts()
      suite = s
      const f = s.seed('typed-account')
      s.assign(f)
      const before = s.evidence(f)
      const body =
        'first line\n\nLiteral $(no-shell), `quoted`, unicode 雪 and NUL-free bytes\n' +
        'body line\n'.repeat(300)
      s.clear()
      s.update({ echo_credential: true })
      for (const action of [
        {
          type: 'create_pull_request',
          title: 'Offline PR',
          body,
          base: 'main',
          head: 'fork:feature',
          draft: true,
        },
        { type: 'create_issue', title: 'Offline issue', body },
      ])
        expect(s.github(f, action).code).toBe(0)
      expect(object(list(s.github(f).data)[0]).body).toBe('[redacted]')
      const typed = s.calls().filter(row => row.kind === 'typed')
      expect(typed).toHaveLength(2)
      const graphql = s
        .calls()
        .filter(row => row.kind === 'api' && list(row.args).includes('graphql'))
      expect(graphql).toHaveLength(2)
      expect(
        graphql.every(row => row.actor === 'alicecase' && row.pinned === true)
      ).toBe(true)
      const payloads = graphql.map(row => object(JSON.parse(string(row.body))))
      const mutation = payloads.find(value =>
        string(value.query).includes('createPullRequest')
      )
      expect(mutation).toBeDefined()
      const input = object(object(object(mutation).variables).input)
      expect(input).toMatchObject({ body, headRefName: 'fork:feature' })
      expect(
        graphql.every(
          row =>
            list(row.args).includes('--input') && list(row.args).at(-1) === '-'
        )
      ).toBe(true)
      expect(
        s
          .calls()
          .some(row => list(row.args).slice(0, 2).join(':') === 'pr:create')
      ).toBe(false)
      for (const row of typed) {
        expect(row).toMatchObject({
          actor: 'alicecase',
          pinned: true,
          debug_cleared: true,
          defaults_cleared: true,
        })
        const args = list(row.args)
        expect(args.slice(-2)).toEqual(['--repo', 'github.com/fork/repo'])
        if (args[1] === 'create') {
          expect(row.body).toBe(body)
          expect(args).toContain('--body-file')
          expect(args).not.toContain(body)
        }
      }
      expect(
        s
          .calls()
          .filter(row => row.kind === 'catalog' || row.kind === 'token')
          .every(row => row.tokens_absent === true)
      ).toBe(true)
      expect(
        s.calls().some(row => {
          const args = list(row.args)
          return (
            args.includes('--user') &&
            args[args.indexOf('--user') + 1] === 'AliceCase'
          )
        })
      ).toBe(true)
      expect(s.evidence(f)).toEqual(before)
      s.clear()
      s.github(f, undefined, 'foreign.example/fork/repo', true)
      expect(s.calls()).toHaveLength(0)
      const assignment = s.account(f).assignment
      const stored = readFileSync(s.registry)
      s.update({ echo_credential: false, signed_out: ['AliceCase'] })
      s.clear()
      s.github(
        f,
        { type: 'create_issue', title: 'Must not run', body },
        null,
        true
      )
      expect(s.calls().some(row => row.kind === 'typed')).toBe(false)
      expect(s.account(f).assignment).toEqual(assignment)
      expect(readFileSync(s.registry)).toEqual(stored)
      s.update({ catalog_failure: true })
      expect(s.catalog(f)).toMatchObject({
        status: 'unavailable',
        account: { assignment },
      })
      expect(readFileSync(s.registry)).toEqual(stored)
      s.update({ catalog_failure: false, visible: [] })
      expect(s.catalog(f).accounts).toEqual([])
      expect(s.account(f).assignment).toEqual(assignment)
      expect(readFileSync(s.registry)).toEqual(stored)
      clean(s.calls())
      s.configure()
      const g = s.seed('broken-saved-account-scope')
      s.assign(g)
      s.cli(['repositories', 'select', g.root])
      const subdir = path.join(g.root, 'inside', 'working')
      mkdirSync(subdir, { recursive: true })
      const request = JSON.stringify({
        repository: 'github.com/fork/repo',
        action: { type: 'issues' },
      })
      const gBefore = s.evidence(g)
      const metadata = path.join(g.root, '.git')
      const hidden = path.join(g.root, '.git-account-test-hidden')
      renameSync(metadata, hidden)
      s.clear()
      try {
        s.cli(['--selected', 'github', request], { fail: true })
        s.cli(['-C', subdir, 'github', request], { fail: true })
        expect(s.calls()).toHaveLength(0)
      } finally {
        renameSync(hidden, metadata)
      }
      expect(s.evidence(g)).toEqual(gBefore)
      const outside = path.join(s.root, 'outside-unsaved-git')
      mkdirSync(outside)
      expect(s.cli(['-C', outside, 'github', request]).code).toBe(0)
      expect(
        s
          .calls()
          .filter(row => row.kind === 'typed')
          .at(-1)?.actor
      ).toBe('environment-user')
      const corrupt = path.join(s.root, 'invalid-native-namespace.json')
      privateFile(corrupt, '{broken')
      s.clear()
      s.cli(['-C', outside, 'github', request], { config: corrupt, fail: true })
      expect(s.calls()).toHaveLength(0)
      expect(readFileSync(corrupt, 'utf8')).toBe('{broken')
    }, 180_000)

    it('rejects queued mutations after the account or repository binding changes', () => {
      const s = new Accounts()
      suite = s
      const requests = [
        {
          action: {
            type: 'create_pull_request',
            title: 'Queued offline PR',
            body: 'queued PR body\n',
            base: 'main',
            head: 'fork:feature',
          },
        },
        {
          action: {
            type: 'create_issue',
            title: 'Queued offline issue',
            body: 'queued issue body\n',
          },
        },
      ]
      for (const [label, target] of [
        ['named', OTHER],
        ['none', NONE],
      ] as const) {
        s.configure()
        const f = s.seed(`queued-account-${label}`)
        s.assign(f)
        const basis = s.account(f).basis
        s.assign(f, target)
        const current = s.account(f)
        const stored = readFileSync(s.registry)
        const before = s.evidence(f)
        s.clear()
        for (const request of requests)
          s.rpc(
            f,
            'github',
            { request: { ...request, account_basis: basis } },
            { fail: true }
          )
        expect(s.calls()).toHaveLength(0)
        expect(readFileSync(s.registry)).toEqual(stored)
        expect(s.evidence(f)).toEqual(before)
        if (label === 'named') {
          for (const request of requests)
            expect(
              s.rpc(f, 'github', {
                request: { ...request, account_basis: current.basis },
              }).code
            ).toBe(0)
          const typed = s.calls().filter(row => row.kind === 'typed')
          expect(typed).toHaveLength(1)
          expect(
            typed.every(row => row.actor === 'bobcase' && row.pinned === true)
          ).toBe(true)
          const graphql = s
            .calls()
            .filter(
              row => row.kind === 'api' && list(row.args).includes('graphql')
            )
          expect(graphql).toHaveLength(2)
          expect(
            graphql.every(row => row.actor === 'bobcase' && row.pinned === true)
          ).toBe(true)
          expect(
            graphql.some(row =>
              string(object(JSON.parse(string(row.body))).query).includes(
                'createPullRequest'
              )
            )
          ).toBe(true)
          expect(s.github(f).code).toBe(0)
          expect(
            s
              .calls()
              .filter(row => row.kind === 'typed')
              .at(-1)?.actor
          ).toBe('bobcase')
        } else {
          s.rpc(
            f,
            'github',
            { request: { ...requests[0], account_basis: current.basis } },
            { fail: true }
          )
          expect(s.calls()).toHaveLength(0)
        }
        expect(readFileSync(s.registry)).toEqual(stored)
        expect(s.evidence(f)).toEqual(before)
        s.clear()
        expect(
          s.rpc(f, 'github', {
            request: { account_basis: basis, action: { type: 'auth_status' } },
          }).code
        ).toBe(0)
        expect(s.calls().map(row => row.kind)).toEqual(['catalog'])
        expect(readFileSync(s.registry)).toEqual(stored)
        expect(s.evidence(f)).toEqual(before)
      }
      s.configure()
      const f = s.seed('queued-account-source')
      s.assign(f)
      const basis = s.account(f).basis
      const account = object(s.record(f).account)
      const before = s.evidence(f)
      f.git(
        'remote',
        'set-url',
        'origin',
        'https://github.com/changed/repo.git'
      )
      try {
        expect(s.account(f).basis).not.toBe(basis)
        expect(object(s.record(f).account)).toEqual(account)
        const stored = readFileSync(s.registry)
        const changed = s.evidence(f)
        s.clear()
        for (const request of requests)
          s.rpc(
            f,
            'github',
            { request: { ...request, account_basis: basis } },
            { fail: true }
          )
        expect(s.calls()).toHaveLength(0)
        expect(readFileSync(s.registry)).toEqual(stored)
        expect(s.evidence(f)).toEqual(changed)
      } finally {
        f.git('remote', 'set-url', 'origin', 'https://github.com/fork/repo.git')
      }
      expect(s.account(f).basis).toBe(basis)
      expect(s.evidence(f)).toEqual(before)
      const outside = path.join(s.root, 'queued-account-outside')
      mkdirSync(outside)
      const stored = readFileSync(s.registry)
      s.clear()
      s.cli(
        [
          '-C',
          outside,
          'github',
          JSON.stringify({
            ...requests[0],
            account_basis: basis,
            repository: 'github.com/fork/repo',
          }),
        ],
        { fail: true }
      )
      expect(s.calls()).toHaveLength(0)
      expect(readFileSync(s.registry)).toEqual(stored)
      expect(s.evidence(f)).toEqual(before)
    }, 180_000)

    it('rejects stale account reviews and cross-namespace authorization', () => {
      const s = new Accounts()
      suite = s
      for (const change of [
        'actor',
        'repository',
        'source',
        'account',
        'record',
      ]) {
        s.configure()
        const f = s.seed(`stale-account-${change}`)
        const plan = s.preview(f)
        if (change === 'actor') {
          const users = object(structuredClone(s.data.users))
          users.AliceCase = { ...object(users.AliceCase), id: 999 }
          s.update({ users })
        } else if (change === 'repository') {
          const repositories = object(structuredClone(s.data.repositories))
          repositories.alicecase = {
            ...object(repositories.alicecase),
            id: 9001,
          }
          s.update({ repositories })
        } else if (change === 'source')
          f.git(
            'remote',
            'set-url',
            'origin',
            'https://github.com/changed/repo.git'
          )
        else if (change === 'account') s.assign(f, NONE)
        else {
          s.cli(['repositories', 'forget', f.root])
          s.cli(['repositories', 'add', f.root])
        }
        const stored = readFileSync(s.registry)
        const before = s.evidence(f)
        s.rpc(
          f,
          'set_repository_account',
          { target: NAMED, basis: plan.basis },
          { fail: true }
        )
        expect(readFileSync(s.registry)).toEqual(stored)
        expect(s.evidence(f)).toEqual(before)
      }
      s.configure()
      const f = s.seed('namespace-account')
      const plan = s.preview(f)
      const other = path.join(s.root, 'other-namespace.json')
      privateFile(other, readFileSync(s.registry))
      const before = readFileSync(other)
      s.rpc(
        f,
        'set_repository_account',
        { target: NAMED, basis: plan.basis },
        { config: other, fail: true }
      )
      expect(readFileSync(other)).toEqual(before)
      s.assign(f)
      const assigned = s.account(f)
      const subdir = path.join(f.root, 'nested', 'working')
      mkdirSync(subdir, { recursive: true })
      const nested = s.account(f.at(subdir))
      expect(nested).toMatchObject({
        scope_id: assigned.scope_id,
        assignment: assigned.assignment,
      })
      expect(
        s.rpc(f, 'repository_account', {}, { config: other }).assignment
      ).toEqual(EFFECTIVE)
      const stored = readFileSync(s.registry)
      s.rpc(
        f,
        'repository_account_preview',
        { target: { ...NAMED, id: 999 } },
        { fail: true }
      )
      expect(readFileSync(s.registry)).toEqual(stored)
    }, 180_000)

    it('rejects malformed saved records and leaves legacy records unchanged', () => {
      const s = new Accounts()
      suite = s
      const f = s.seed('malformed-account-record')
      const original = object(JSON.parse(readFileSync(s.registry, 'utf8')))
      for (const fault of [
        'account',
        'assignment',
        'mode',
        'unknown',
        'effective_extra',
        'none_extra',
      ]) {
        const value = object(structuredClone(original))
        const records = list(value.records).map(object)
        const record = records.find(item => item.path === f.root)
        if (record === undefined) throw new Error('missing saved account')
        const account = object(record.account)
        if (fault === 'account') delete record.account
        else {
          if (fault === 'assignment') delete account.assignment
          else if (fault === 'mode')
            account.assignment = { mode: 'unrecognized' }
          else if (fault === 'effective_extra')
            account.assignment = {
              mode: 'effective',
              login: 'untrusted',
              token: SECRET,
            }
          else if (fault === 'none_extra')
            account.assignment = { mode: 'none', allow: true }
          else account.unknown = SECRET
          record.account = account
        }
        value.records = records
        const encoded = JSON.stringify(value)
        privateFile(s.registry, encoded)
        s.clear()
        s.rpc(f, 'repository_account', {}, { fail: true })
        s.github(f, undefined, null, true)
        expect(readFileSync(s.registry, 'utf8')).toBe(encoded)
        expect(s.calls()).toHaveLength(0)
      }
      privateFile(s.registry, JSON.stringify(original))
      s.clear()
      const before = readFileSync(s.registry)
      for (const target of [
        { mode: 'none', allow: true },
        { mode: 'effective', login: 'untrusted', token: SECRET },
      ]) {
        s.rpc(f, 'repository_account_preview', { target }, { fail: true })
        s.rpc(
          f,
          'set_repository_account',
          { target, basis: '0'.repeat(64) },
          { fail: true }
        )
      }
      expect(s.calls()).toHaveLength(0)
      expect(readFileSync(s.registry)).toEqual(before)
      const g = s.seed('legacy-account', false)
      const legacy = path.join(s.root, 'legacy-account.json')
      const encoded = JSON.stringify({
        version: 1,
        selected: g.root,
        repositories: [g.root],
      })
      privateFile(legacy, encoded)
      const account = s.rpc(g, 'repository_account', {}, { config: legacy })
      expect(account).toMatchObject({ saved: true, assignment: EFFECTIVE })
      expect(string(account.scope_id)).toMatch(/^legacy-/)
      s.rpc(g, 'repository_account_catalog', {}, { config: legacy })
      s.rpc(g, 'snapshot', {}, { config: legacy })
      expect(readFileSync(legacy, 'utf8')).toBe(encoded)
    }, 180_000)
    it('keeps linked-worktree account scopes distinct and adopts only related worktrees', async () => {
      const s = new Accounts()
      suite = s
      const f = s.seed('shared-main')
      s.assign(f)
      const alice = s.account(f)
      const linked = path.join(s.root, 'shared-linked')
      f.git('worktree', 'add', '-qb', 'linked-account', linked)
      const g = f.at(linked)
      s.cli(['repositories', 'add', linked])
      s.assign(g, OTHER)
      const bob = s.account(g)
      expect(alice.scope_id).not.toBe(bob.scope_id)
      expect(object(bob.assignment).id).toBe(202)
      expect(object(s.rpc(f, 'repository_context').repository).is_fork).toBe(
        true
      )
      expect(object(s.rpc(g, 'repository_context').repository).is_fork).toBe(
        false
      )
      const first = s.record(f)
      const second = s.record(g)
      const third = path.join(s.root, 'shared-unsaved')
      f.git('worktree', 'add', '-qb', 'third-account', third)
      const bridge = await AccountBridge.start(s, f)
      try {
        const adopted = await bridge.call('open_worktree', { path: linked })
        expect(object(adopted.repository_account)).toMatchObject({
          scope_id: bob.scope_id,
          assignment: { id: 202 },
        })
        expect(s.record(f)).toEqual(first)
        expect(s.record(g)).toEqual(second)
        const moved = await bridge.call('open_worktree', { path: third })
        expect(object(moved.repository_account)).toMatchObject({
          scope_id: bob.scope_id,
          assignment: { id: 202 },
        })
        const h = f.at(third)
        expect(s.record(h)).toMatchObject({
          id: second.id,
          account: second.account,
        })
        expect(s.record(f)).toEqual(first)
        expect(s.account(g).saved).toBe(false)
        expect(object(s.rpc(h, 'repository_context').repository).is_fork).toBe(
          false
        )
        expect(object(s.rpc(f, 'repository_context').repository).is_fork).toBe(
          true
        )
        const unrelated = s.seed('unrelated-worktree-target')
        const before = readFileSync(s.registry)
        await bridge.call('open_worktree', { path: unrelated.root }, true)
        expect(readFileSync(s.registry)).toEqual(before)
      } finally {
        await bridge.close()
      }
    }, 180_000)

    it('rejects forgotten bridge handles until the repository is reopened', async () => {
      const s = new Accounts()
      suite = s
      const f = s.seed('forgotten-fixed-handle')
      s.assign(f)
      const old = s.account(f)
      const linked = path.join(s.root, 'forgotten-linked')
      f.git('worktree', 'add', '-qb', 'forgotten-linked', linked)
      const bridge = await AccountBridge.start(s, f)
      try {
        expect((await bridge.call('repository_account')).scope_id).toBe(
          old.scope_id
        )
        s.cli(['repositories', 'forget', f.root])
        s.cli(['repositories', 'add', f.root])
        expect(s.account(f).scope_id).not.toBe(old.scope_id)
        const stored = readFileSync(s.registry)
        s.clear()
        await bridge.call('repository_account', {}, true)
        await bridge.call(
          'github',
          { request: { action: { type: 'issues' } } },
          true
        )
        await bridge.call('open_worktree', { path: linked }, true)
        expect(s.calls()).toHaveLength(0)
        expect(readFileSync(s.registry)).toEqual(stored)
        const rebound = await bridge.call('open_repository', { path: f.root })
        const account = object(rebound.repository_account)
        expect(account.scope_id).not.toBe(old.scope_id)
        expect(account.assignment).toEqual(EFFECTIVE)
      } finally {
        await bridge.close()
      }
    }, 180_000)
    it('fences delayed account discovery and concurrent assignment decisions', async () => {
      const s = new Accounts()
      suite = s
      const wait = async (
        result: Promise<number | null>
      ): Promise<number | null> => {
        let timer: ReturnType<typeof setTimeout> | undefined
        try {
          return await Promise.race([
            result,
            new Promise<never>((_, reject) => {
              timer = setTimeout(
                () => reject(new Error('Delayed account request timed out')),
                15_000
              )
            }),
          ])
        } finally {
          if (timer !== undefined) clearTimeout(timer)
        }
      }
      const start = async (
        repo: { readonly root: string },
        method: string,
        params: Record<string, unknown>
      ) => {
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
        child.stdin.end(JSON.stringify({ method, ...params }))
        const deadline = Date.now() + 10_000
        while (
          !existsSync(s.started) &&
          Date.now() < deadline &&
          child.exitCode === null
        )
          await new Promise(resolve => setTimeout(resolve, 20))
        if (!existsSync(s.started)) {
          child.kill('SIGKILL')
          await finished
          throw new Error('Delayed account request did not start')
        }
        return { child, finished, output: () => ({ stdout, stderr }) }
      }
      for (const [label, delay, method, params, expectedTyped] of [
        [
          'catalog-source',
          { kind: 'catalog' },
          'repository_account_catalog',
          {},
          0,
        ],
        [
          'typed-opening-source',
          { kind: 'api', endpoint: '/user' },
          'github',
          {
            request: {
              action: {
                type: 'create_issue',
                title: 'Must not run',
                body: 'offline body',
              },
            },
          },
          0,
        ],
        [
          'typed-response-source',
          { kind: 'typed' },
          'github',
          { request: { action: { type: 'issues' } } },
          1,
        ],
      ] as const) {
        s.configure()
        const f = s.seed(label)
        s.assign(f)
        s.clear()
        s.update({ delay })
        const running = await start(f, method, params)
        try {
          f.git(
            'remote',
            'set-url',
            'origin',
            'https://github.com/changed/repo.git'
          )
          const stored = readFileSync(s.registry)
          const before = s.evidence(f)
          writeFileSync(s.release, 'release')
          expect(await wait(running.finished)).not.toBe(0)
          const output = running.output()
          clean(output.stdout)
          clean(output.stderr)
          expect(readFileSync(s.registry)).toEqual(stored)
          expect(s.evidence(f)).toEqual(before)
          expect(s.calls().filter(row => row.kind === 'typed')).toHaveLength(
            expectedTyped
          )
        } finally {
          if (
            running.child.exitCode === null &&
            running.child.signalCode === null
          ) {
            running.child.kill('SIGKILL')
            await running.finished
          }
        }
      }
      s.configure()
      const f = s.seed('pending-assignment-cas')
      const plan = s.preview(f)
      s.update({ delay: { kind: 'api', endpoint: '/user' } })
      const running = await start(f, 'set_repository_account', {
        target: NAMED,
        basis: plan.basis,
      })
      try {
        s.assign(f, NONE)
        const stored = readFileSync(s.registry)
        const before = s.evidence(f)
        writeFileSync(s.release, 'release')
        expect(await wait(running.finished)).not.toBe(0)
        const output = running.output()
        clean(output.stdout)
        clean(output.stderr)
        expect(readFileSync(s.registry)).toEqual(stored)
        expect(s.account(f).assignment).toEqual(NONE)
        expect(object(s.record(f).account).association).toBeNull()
        expect(s.evidence(f)).toEqual(before)
      } finally {
        if (
          running.child.exitCode === null &&
          running.child.signalCode === null
        ) {
          running.child.kill('SIGKILL')
          await running.finished
        }
      }
    }, 180_000)
  }
)
