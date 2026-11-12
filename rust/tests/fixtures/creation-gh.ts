/** Scoped gh fixture forwarding API calls to an isolated TLS service. */
import { spawnSync } from 'node:child_process'
import { appendFileSync, readFileSync } from 'node:fs'
import { request } from 'node:https'

interface IUser {
  readonly id: number
  readonly login: string
  readonly token: string
}
interface IRepository {
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
  readonly api_exits?: Record<string, number>
  readonly api_exit?: number
  readonly typed_code?: number
}
const required = (name: string): string => {
  const value = process.env[name]
  if (value === undefined) throw new Error(`Missing ${name}`)
  return value
}
const scenario: IScenario = JSON.parse(
  readFileSync(required('CONTEXT_SCENARIO'), 'utf8')
)
const args = process.argv.slice(2)
const body = readFileSync(0)
const secrets = [
  ...Object.values(scenario.users).map(user => user.token),
  scenario.environment.token,
  scenario.secret,
]
const safe = (value: string): string =>
  secrets.reduce(
    (result, secret) => result.replaceAll(secret, '[redacted]'),
    value
  )
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
const tokenNames = [
  'GH_TOKEN',
  'GITHUB_TOKEN',
  'GH_ENTERPRISE_TOKEN',
  'GITHUB_ENTERPRISE_TOKEN',
]
const row = {
  args: args.map(safe),
  kind,
  actor: actor?.login ?? null,
  body: safe(body.toString('utf8')),
  argv_clean: args.every(arg => secrets.every(secret => !arg.includes(secret))),
  stdin_clean: secrets.every(secret => !body.includes(Buffer.from(secret))),
  tokens_absent: tokenNames.every(name => process.env[name] === undefined),
  pinned:
    actor !== undefined &&
    tokenNames.every(name => process.env[name] === token),
  debug_cleared:
    process.env.GH_DEBUG === undefined && process.env.DEBUG === undefined,
  defaults_cleared:
    process.env.GH_HOST === undefined && process.env.GH_REPO === undefined,
  native_api: kind === 'api' && Boolean(process.env.CREATE_REAL_GH),
}
appendFileSync(required('CONTEXT_GH_LOG'), `${JSON.stringify(row)}\n`)
if (kind === 'protocol') {
  process.stdout.write(`${scenario.git_protocol ?? 'https'}\n`)
  process.exit(0)
}
if (kind === 'token') {
  if (
    selectedLogin !== undefined &&
    (!scenario.visible.includes(selectedLogin) ||
      scenario.signed_out.includes(selectedLogin))
  ) {
    process.stderr.write(scenario.secret)
    process.exit(1)
  }
  process.stdout.write(`${token}\n`)
  process.exit(0)
}
if (kind === 'catalog') {
  const accounts = scenario.visible.map(name => ({
    host,
    login: name,
    active: name === scenario.active,
    state: scenario.signed_out.includes(name) ? 'error' : 'success',
  }))
  process.stdout.write(`${JSON.stringify({ hosts: { [host]: accounts } })}\n`)
  process.exit(0)
}
if (kind === 'metadata') {
  if (actor === undefined) process.exit(4)
  const repo = scenario.repositories[actor.login]
  const parent = repo.parent
  process.stdout.write(
    `${JSON.stringify({ nameWithOwner: repo.full_name, url: repo.html_url, isFork: repo.fork, parent: parent === null ? null : { nameWithOwner: parent.full_name, url: parent.html_url } })}\n`
  )
  process.exit(0)
}
if (kind === 'api') {
  const query: string = body.length
    ? (JSON.parse(body.toString('utf8')).query ?? '')
    : ''
  const stage = query.includes('createPullRequest')
    ? 'mutation'
    : query
      ? 'lookup'
      : args.includes('/user')
        ? 'user'
        : 'repository'
  const apiExit = scenario.api_exits?.[stage] ?? scenario.api_exit ?? 0
  const real = process.env.CREATE_REAL_GH
  if (real) {
    const env = { ...process.env }
    for (const name of Object.keys(env))
      if (
        ['HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY'].includes(name.toUpperCase())
      )
        delete env[name]
    Object.assign(env, {
      SSL_CERT_FILE: required('CREATE_API_CA'),
      HTTPS_PROXY: required('CREATE_API_PROXY'),
      HTTP_PROXY: required('CREATE_API_PROXY'),
      NO_PROXY: '',
      no_proxy: '',
      CREATE_FROM_GH: '1',
    })
    const result = spawnSync(real, args, {
      input: body,
      env,
      stdio: ['pipe', 'inherit', 'inherit'],
    })
    if (result.error !== undefined || result.status === null) process.exit(1)
    process.exit(result.status || apiExit)
  }
  let endpoint: string | undefined
  for (let i = 1; i < args.length; i++) {
    const arg = args[i]
    if (
      [
        '--hostname',
        '--method',
        '--header',
        '--input',
        '--cache',
        '-X',
        '-H',
      ].includes(arg)
    )
      i++
    else if (!arg.startsWith('-') && endpoint === undefined) endpoint = arg
  }
  if (endpoint === undefined) process.exit(94)
  const methodIndex = args.indexOf('--method')
  const method =
    methodIndex < 0 ? (body.length ? 'POST' : 'GET') : args[methodIndex + 1]
  const url = new URL(
    endpoint.replace(/^\/+/, ''),
    `${required('CREATE_API_ENDPOINT')}/`
  )
  const pending = request(
    url,
    {
      method,
      ca: readFileSync(required('CREATE_API_CA')),
      headers: {
        Authorization: `Bearer ${token}`,
        'Content-Type': 'application/json',
      },
      timeout: 20_000,
    },
    response => {
      const chunks: Buffer[] = []
      response.on('data', chunk => chunks.push(Buffer.from(chunk)))
      response.on('end', () => {
        const output = Buffer.concat(chunks)
        process.stdout.write(output)
        if ((response.statusCode ?? 500) >= 400) {
          process.stderr.write(
            Buffer.concat([Buffer.from('fixture HTTP failure '), output])
          )
          process.exitCode = 1
        } else process.exitCode = apiExit
      })
    }
  )
  pending.on('timeout', () =>
    pending.destroy(new Error('fixture API timed out'))
  )
  pending.on('error', () => {
    process.stderr.write('fixture API unavailable')
    process.exitCode = 1
  })
  pending.end(body)
} else if (args[0] === 'pr' && args[1] === 'create') {
  const result = spawnSync('git', ['remote', '-v'], {
    env: { ...process.env, CREATE_FROM_GH: '1' },
    stdio: 'ignore',
  })
  if (result.status !== 0) process.exit(1)
  process.stdout.write('https://github.com/fork/repo/pull/7\n')
  process.exitCode = scenario.typed_code ?? 0
} else {
  process.stdout.write(JSON.stringify({ unexpected: kind }))
  process.exitCode = 96
}
