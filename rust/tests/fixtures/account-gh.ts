/** Offline gh fixture for saved-account integration tests. */
import {
  appendFileSync,
  existsSync,
  readFileSync,
  writeFileSync,
} from 'node:fs'

interface IUser {
  readonly id: number
  readonly login: string
  readonly token: string
}
interface IRepository {
  readonly id: number
  readonly name: string
  readonly full_name: string
  readonly html_url: string
  readonly fork: boolean
  readonly parent: {
    readonly full_name: string
    readonly html_url: string
  } | null
}
interface IScenario {
  readonly host: string
  readonly active: string
  readonly visible: ReadonlyArray<string>
  readonly signed_out: ReadonlyArray<string>
  readonly users: Record<string, IUser>
  readonly environment: IUser
  readonly repositories: Record<string, IRepository>
  readonly secret: string
  readonly git_protocol?: string
  readonly catalog_failure?: boolean
  readonly echo_credential?: boolean
  readonly api_failures?: Record<string, number>
  readonly pull_requests?: Record<string, unknown>
  readonly repository_responses?: Record<string, unknown>
  readonly typed_code?: number
  readonly delay?: { readonly kind: string; readonly endpoint?: string }
}
const required = (name: string): string => {
  const value = process.env[name]
  if (value === undefined) throw new Error(`Missing fixture setting ${name}`)
  return value
}
const scenario: IScenario = JSON.parse(
  readFileSync(required('CONTEXT_SCENARIO'), 'utf8')
)
const args = process.argv.slice(2)
const body = readFileSync(0, 'utf8')
const hostIndex = args.indexOf('--hostname')
const host = hostIndex < 0 ? scenario.host : args[hostIndex + 1]
const kind =
  args[0] === 'config' && args[1] === 'get' && args[2] === 'git_protocol'
    ? 'protocol'
    : args[0] === 'auth' && args[1] === 'token'
      ? 'token'
      : args[0] === 'auth' && args[1] === 'status'
        ? 'catalog'
        : args[0] === 'api'
          ? 'api'
          : args[0] === 'repo' && args[1] === 'view'
            ? 'metadata'
            : 'typed'
const userIndex = args.indexOf('--user')
const selectedLogin = userIndex < 0 ? undefined : args[userIndex + 1]
const tokenVars =
  host === 'github.com' || host.endsWith('.ghe.com')
    ? ['GH_TOKEN', 'GITHUB_TOKEN']
    : ['GH_ENTERPRISE_TOKEN', 'GITHUB_ENTERPRISE_TOKEN']
let token =
  tokenVars.map(name => process.env[name]).find(Boolean) ??
  scenario.users[scenario.active].token
if (kind === 'token' && selectedLogin !== undefined)
  token = scenario.users[selectedLogin]?.token ?? ''
const actor = [...Object.values(scenario.users), scenario.environment].find(
  user => user.token === token
)
const endpoint =
  kind === 'api'
    ? args.includes('graphql')
      ? 'graphql'
      : args.at(-1)
    : undefined
const tokenNames = [
  'GH_TOKEN',
  'GITHUB_TOKEN',
  'GH_ENTERPRISE_TOKEN',
  'GITHUB_ENTERPRISE_TOKEN',
]
const entry = {
  args,
  kind,
  actor: actor?.login ?? null,
  body,
  tokens_absent: tokenNames.every(name => process.env[name] === undefined),
  pinned:
    actor !== undefined &&
    tokenNames.every(name => process.env[name] === token),
  debug_cleared:
    process.env.GH_DEBUG === undefined && process.env.DEBUG === undefined,
  defaults_cleared:
    process.env.GH_HOST === undefined && process.env.GH_REPO === undefined,
}
appendFileSync(required('CONTEXT_GH_LOG'), `${JSON.stringify(entry)}\n`)
if (
  scenario.delay?.kind === kind &&
  (scenario.delay.endpoint === undefined ||
    scenario.delay.endpoint === endpoint)
) {
  writeFileSync(required('CONTEXT_GH_STARTED'), 'started')
  const deadline = Date.now() + 25_000
  while (!existsSync(required('CONTEXT_GH_RELEASE')) && Date.now() < deadline) {
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 20)
  }
}
if (kind === 'protocol') {
  process.stdout.write(`${scenario.git_protocol ?? 'https'}\n`)
  process.exit(0)
}
if (kind === 'catalog') {
  if (scenario.catalog_failure) {
    process.stdout.write(scenario.secret)
    process.stderr.write(scenario.secret)
    process.exit(1)
  }
  const rows = scenario.visible.map(name => ({
    host,
    login: name,
    active: name === scenario.active,
    state: scenario.signed_out.includes(name) ? 'error' : 'success',
    gitProtocol: 'https',
    tokenSource: 'keyring',
    oauthToken: scenario.secret,
    token: scenario.secret,
    unknown: { token: scenario.secret },
  }))
  process.stdout.write(`${JSON.stringify({ hosts: { [host]: rows } })}\n`)
  process.exit(0)
}
if (kind === 'token') {
  if (
    (selectedLogin !== undefined &&
      (!scenario.visible.includes(selectedLogin) ||
        scenario.signed_out.includes(selectedLogin))) ||
    !token
  ) {
    process.stderr.write(scenario.secret)
    process.exit(1)
  }
  process.stdout.write(`${token}\n`)
  process.exit(0)
}
if (actor === undefined) {
  process.stderr.write(scenario.secret)
  process.exit(4)
}
if (kind === 'api') {
  const failure = scenario.api_failures?.[endpoint ?? '']
  if (failure !== undefined && failure !== 0) {
    process.stdout.write(scenario.secret)
    process.stderr.write(scenario.secret)
    process.exit(failure)
  }
  if (!entry.pinned) {
    process.stderr.write(scenario.secret)
    process.exit(93)
  }
  let value: unknown
  if (endpoint === 'graphql') {
    const payload: { query: string } = JSON.parse(body)
    const repo = scenario.repositories[actor.login]
    if (scenario.echo_credential) process.stderr.write(`diagnostic ${token}\n`)
    value = payload.query.includes('createPullRequest')
      ? {
          data: {
            createPullRequest: {
              pullRequest: {
                id: 'PR_fixture',
                url: `https://${scenario.host}/${repo.full_name}/pull/7`,
              },
            },
          },
        }
      : {
          data: {
            repository: {
              id: 'R_fixture',
              nameWithOwner: repo.full_name,
              url: repo.html_url,
              pullRequests: {
                nodes: [],
                pageInfo: { hasNextPage: false, endCursor: null },
              },
            },
          },
        }
  } else if (endpoint === '/user')
    value = { id: actor.id, login: actor.login, plan: { name: 'pro' } }
  else if (
    endpoint !== undefined &&
    scenario.pull_requests?.[endpoint] !== undefined
  )
    value = scenario.pull_requests[endpoint]
  else if (
    endpoint !== undefined &&
    scenario.repository_responses?.[endpoint] !== undefined
  )
    value = scenario.repository_responses[endpoint]
  else if (endpoint?.startsWith('/repos/') && !endpoint.includes('/rules/'))
    value = scenario.repositories[actor.login]
  else if (endpoint?.includes('/rules/branches/')) value = []
  else {
    process.stderr.write(scenario.secret)
    process.exit(94)
  }
  process.stdout.write(`${JSON.stringify(value)}\n`)
  process.exit(0)
}
if (kind === 'metadata') {
  const repo = scenario.repositories[actor.login]
  const parent = repo.parent
  process.stdout.write(
    `${JSON.stringify({ nameWithOwner: repo.full_name, url: repo.html_url, isFork: repo.fork, parent: parent === null ? null : { nameWithOwner: parent.full_name, url: parent.html_url } })}\n`
  )
  process.exit(0)
}
if (scenario.echo_credential) process.stderr.write(`diagnostic ${token}\n`)
if ((args[0] === 'pr' || args[0] === 'issue') && args[1] === 'create') {
  const item = args[0] === 'pr' ? 'pull/7' : 'issues/8'
  process.stdout.write(
    `https://${scenario.host}/fork/repo/${item}${scenario.echo_credential ? `?trace=${token}` : ''}\n`
  )
} else if (args.includes('--json')) {
  const item = {
    number: 8,
    title: 'offline issue',
    body: scenario.echo_credential ? token : 'safe',
  }
  process.stdout.write(
    `${JSON.stringify(args[1] === 'list' ? [item] : item)}\n`
  )
} else process.stdout.write('offline typed operation\n')
process.exit(scenario.typed_code ?? 0)
