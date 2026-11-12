// @vitest-environment node
import { execFile } from 'node:child_process'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { createServer, type Server } from 'node:https'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { promisify } from 'node:util'
import { describe, expect, it } from 'vitest'

const exec = promisify(execFile)
const binary = path.resolve('rust/target/debug/desktop-rs')
const enabled = process.env.DESKTOP_TEST_REAL_GH === '1'

interface IRequestBody {
  readonly query: string
  readonly variables?: { readonly input?: unknown }
}

describe.skipIf(!enabled)('installed gh against a local HTTPS API', () => {
  it('reads GitHub data, creates an issue and refreshes fork context offline', async () => {
    const root = await mkdtemp(path.join(tmpdir(), 'desktop-real-gh-'))
    let server: Server | undefined
    try {
      const cert = path.join(root, 'cert.pem')
      const key = path.join(root, 'key.pem')
      await exec(
        process.env.OPENSSL_BIN ?? 'openssl',
        [
          'req',
          '-x509',
          '-newkey',
          'rsa:2048',
          '-sha256',
          '-nodes',
          '-days',
          '1',
          '-keyout',
          key,
          '-out',
          cert,
          '-subj',
          '/CN=localhost',
          '-addext',
          'subjectAltName=DNS:localhost,IP:127.0.0.1',
        ],
        { timeout: 20_000 }
      )
      const requests: Array<{
        method: string
        path: string
        body?: IRequestBody
      }> = []
      const connection = (nodes: ReadonlyArray<unknown>) => ({
        nodes,
        edges: nodes.map(node => ({ node })),
        totalCount: nodes.length,
        pageInfo: { hasNextPage: false, endCursor: '' },
      })
      const repository = {
        id: 'R_fixture',
        name: 'repo',
        nameWithOwner: 'owner/repo',
        url: 'https://github.example/owner/repo',
        description: 'Local fixture',
        owner: { login: 'owner' },
        defaultBranchRef: { name: 'main' },
        isPrivate: false,
        isFork: false,
        parent: null as unknown,
        viewerPermission: 'WRITE',
        hasIssuesEnabled: true,
        pullRequests: connection([
          {
            number: 7,
            title: 'Fixture PR',
            url: 'https://github.example/owner/repo/pull/7',
            state: 'OPEN',
            isDraft: true,
            headRefName: 'feature',
            baseRefName: 'main',
            author: { login: 'tester' },
            updatedAt: '2026-01-01T00:00:00Z',
            reviewDecision: 'REVIEW_REQUIRED',
          },
        ]),
        issues: connection([
          {
            number: 8,
            title: 'Fixture issue',
            url: 'https://github.example/owner/repo/issues/8',
            state: 'OPEN',
            author: { login: 'tester' },
            labels: { nodes: [] },
            updatedAt: '2026-01-01T00:00:00Z',
          },
        ]),
      }
      server = createServer(
        { cert: await readFile(cert), key: await readFile(key) },
        async (req, res) => {
          try {
            const chunks: Buffer[] = []
            for await (const chunk of req) chunks.push(Buffer.from(chunk))
            const body: IRequestBody | undefined = chunks.length
              ? JSON.parse(Buffer.concat(chunks).toString())
              : undefined
            requests.push({
              method: req.method ?? '',
              path: req.url ?? '',
              body,
            })
            let result: unknown
            if (req.method === 'POST') {
              const query = body?.query ?? ''
              if (query.includes('__type')) {
                result = {
                  data: {
                    Issue: {
                      fields: ['id', 'title', 'body', 'url', 'number'].map(
                        name => ({ name })
                      ),
                    },
                  },
                }
              } else if (
                query.includes('mutation') &&
                query.includes('createIssue')
              ) {
                result = {
                  data: {
                    createIssue: {
                      issue: {
                        url: 'https://github.example/owner/repo/issues/9',
                      },
                    },
                  },
                }
              } else result = { data: { repository } }
            } else if (req.url?.includes('/actions/runs')) {
              result = {
                total_count: 1,
                workflow_runs: [
                  {
                    id: 99,
                    name: 'CI',
                    display_title: 'Fixture run',
                    status: 'completed',
                    conclusion: 'success',
                    head_branch: 'main',
                    head_sha: 'a'.repeat(40),
                    html_url: 'https://github.example/run/99',
                    updated_at: '2026-01-01T00:00:00Z',
                  },
                ],
              }
            } else {
              result = {
                id: 1,
                name: 'repo',
                full_name: 'owner/repo',
                default_branch: 'main',
                owner: { login: 'owner' },
                html_url: 'https://github.example/owner/repo',
              }
            }
            res.writeHead(200, {
              'Content-Type': 'application/json',
              'X-GitHub-Enterprise-Version': '3.17.0',
            })
            res.end(JSON.stringify(result))
          } catch {
            res.writeHead(500).end()
          }
        }
      )
      const apiServer = server
      await new Promise<void>((resolve, reject) => {
        apiServer.once('error', reject)
        apiServer.listen(0, '127.0.0.1', resolve)
      })
      const address = server.address()
      if (!address || typeof address === 'string')
        throw new Error('missing fixture address')
      const hostname = `localhost:${address.port}`
      const env = { ...process.env }
      for (const name of [
        'GH_TOKEN',
        'GITHUB_TOKEN',
        'GH_ENTERPRISE_TOKEN',
        'GITHUB_ENTERPRISE_TOKEN',
      ])
        delete env[name]
      Object.assign(env, {
        HOME: root,
        GH_HOST: hostname,
        GH_ENTERPRISE_TOKEN: 'fixture-token',
        GH_CONFIG_DIR: path.join(root, 'gh-config'),
        SSL_CERT_FILE: cert,
        GH_NO_UPDATE_NOTIFIER: '1',
        GH_NO_EXTENSION_UPDATE_NOTIFIER: '1',
        GIT_CONFIG_NOSYSTEM: '1',
        GIT_CONFIG_GLOBAL: path.join(root, 'global.config'),
        GH_REPO: 'wrong.invalid/wrong/repository',
      })
      const cli = async (dir: string, ...args: string[]) => {
        const { stdout } = await exec(
          binary,
          ['--config', path.join(root, 'registry.json'), '-C', dir, ...args],
          { env, timeout: 20_000 }
        )
        return JSON.parse(stdout)
      }
      const run = (action: object) =>
        cli(
          root,
          'github',
          JSON.stringify({ repository: `${hostname}/owner/repo`, action })
        )
      expect((await run({ type: 'repository' })).data.nameWithOwner).toBe(
        'owner/repo'
      )
      expect((await run({ type: 'pull_requests' })).data[0].number).toBe(7)
      expect((await run({ type: 'issues' })).data[0].number).toBe(8)
      expect((await run({ type: 'workflow_runs' })).data[0].databaseId).toBe(99)
      const body = 'Literal $(not-shell)\n\nMultiline body'
      expect(
        (await run({ type: 'create_issue', title: 'Offline issue', body }))
          .stdout
      ).toContain('issues/9')
      const mutations = requests.filter(request =>
        request.body?.query?.includes('createIssue')
      )
      expect(mutations.length).toBeGreaterThan(0)
      expect(mutations.at(-1)?.body?.variables?.input).toMatchObject({
        title: 'Offline issue',
        body,
      })

      const repo = path.join(root, 'worktree')
      await exec('git', ['init', '-qb', 'main', repo], { env, timeout: 20_000 })
      const git = async (...args: string[]) =>
        (await exec('git', ['-C', repo, ...args], { env, timeout: 20_000 }))
          .stdout
      await git('config', 'user.name', 'Context fixture')
      await git('config', 'user.email', 'context@example.invalid')
      await git('config', 'commit.gpgsign', 'false')
      await writeFile(path.join(repo, 'file'), 'base')
      await git('add', '--', 'file')
      await git('commit', '-qm', 'base')
      await git('branch', 'trunk')
      await git(
        'remote',
        'add',
        'origin',
        `https://private:secret@${hostname}/owner/repo.git?token=secret`
      )
      await git(
        'remote',
        'add',
        'upstream',
        `https://${hostname}/parent/repo.git`
      )
      const tip = (await git('rev-parse', 'HEAD')).trim()
      for (const [name, branch] of [
        ['origin', 'main'],
        ['upstream', 'trunk'],
      ]) {
        await git('update-ref', `refs/remotes/${name}/${branch}`, tip)
        await git(
          'symbolic-ref',
          `refs/remotes/${name}/HEAD`,
          `refs/remotes/${name}/${branch}`
        )
      }
      Object.assign(repository, {
        url: `https://${hostname}/owner/repo`,
        isFork: true,
        parent: {
          id: 'R_parent',
          name: 'repo',
          nameWithOwner: 'parent/repo',
          owner: { login: 'parent' },
          url: `https://${hostname}/parent/repo`,
        },
      })
      const metadata = await cli(repo, 'repository-context', '--refresh')
      expect(metadata.status).toBe('verified')
      expect(metadata.repository.parent.owner).toBe('parent')
      expect(JSON.stringify(metadata)).not.toContain('secret')
      const count = requests.length
      expect((await cli(repo, 'branch-policy')).default_branch.name).toBe(
        'trunk'
      )
      expect(requests).toHaveLength(count)
    } finally {
      const apiServer = server
      if (apiServer?.listening)
        await new Promise<void>((resolve, reject) =>
          apiServer.close(error => (error ? reject(error) : resolve()))
        )
      await rm(root, { recursive: true, force: true })
    }
  }, 120_000)
})
