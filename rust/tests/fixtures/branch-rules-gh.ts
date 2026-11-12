/** Offline gh fixture used by the branch-name rules integration suite. */
import {
  appendFileSync,
  existsSync,
  readFileSync,
  writeFileSync,
} from 'node:fs'

interface IScenario {
  host: string
  prefix: string
  metadata: unknown
  user: { id: number; login: string }
  repository: unknown
  rules: ReadonlyArray<unknown>
  rulesets: Record<string, unknown>
  expected_token: string
  other_token: string
  secret: string
  pages?: Record<string, ReadonlyArray<unknown>>
  token_raw?: string
  no_account?: boolean
  rotate_token?: boolean
  change_actor_on_second_user?: boolean
  delay_endpoint?: string
  failures?: Record<string, { raw?: string; code?: number }>
}

const required = (name: string): string => {
  const value = process.env[name]
  if (value === undefined) throw new Error(`Missing fixture setting ${name}`)
  return value
}
const scenario: IScenario = JSON.parse(
  readFileSync(required('CONTEXT_SCENARIO'), 'utf8')
)
const tokenFile = required('RULE_TOKEN_FILE')
const args = process.argv.slice(2)
const hostIndex = args.indexOf('--hostname')
const host = hostIndex < 0 ? scenario.host : args[hostIndex + 1]
const tokenVars =
  host === 'github.com' || host.endsWith('.ghe.com')
    ? ['GH_TOKEN', 'GITHUB_TOKEN']
    : ['GH_ENTERPRISE_TOKEN', 'GITHUB_ENTERPRISE_TOKEN']
const selectedToken =
  tokenVars.map(name => process.env[name]).find(Boolean) ??
  readFileSync(tokenFile, 'utf8')
const kind =
  args[0] === 'repo' && args[1] === 'view'
    ? 'metadata'
    : args[0] === 'auth' && args[1] === 'token'
      ? 'token'
      : args[0] === 'auth' && args[1] === 'status'
        ? 'status'
        : args[0] === 'api'
          ? 'api'
          : 'unexpected'
const endpoint = kind === 'api' ? args.at(-1) : undefined
const pinned = [
  'GH_TOKEN',
  'GITHUB_TOKEN',
  'GH_ENTERPRISE_TOKEN',
  'GITHUB_ENTERPRISE_TOKEN',
].every(name => process.env[name] === scenario.expected_token)
const entry = {
  args,
  kind,
  ...(kind === 'metadata'
    ? {}
    : {
        debug_cleared:
          process.env.GH_DEBUG === undefined && process.env.DEBUG === undefined,
        repo_cleared: process.env.GH_REPO === undefined,
        host_cleared: process.env.GH_HOST === undefined,
      }),
  ...(kind === 'api'
    ? {
        pinned,
        store_still_original:
          readFileSync(tokenFile, 'utf8') === scenario.expected_token,
      }
    : {}),
}
appendFileSync(required('CONTEXT_GH_LOG'), `${JSON.stringify(entry)}\n`)
if (kind === 'unexpected') process.exit(96)
if (kind === 'api' && !pinned) {
  process.stderr.write(scenario.secret)
  process.exit(95)
}
if (kind === 'api' && scenario.delay_endpoint === endpoint) {
  writeFileSync(required('CONTEXT_GH_STARTED'), 'started')
  const deadline = Date.now() + 25_000
  while (!existsSync(required('CONTEXT_GH_RELEASE')) && Date.now() < deadline) {
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 20)
  }
}
const failure = scenario.failures?.[kind === 'api' ? (endpoint ?? '') : kind]
if (failure !== undefined) {
  process.stderr.write(scenario.secret)
  process.stdout.write(failure.raw ?? scenario.secret)
  process.exit(failure.code ?? 1)
}
let value: unknown
if (kind === 'metadata') value = scenario.metadata
else if (kind === 'token') {
  process.stdout.write(`${scenario.token_raw ?? selectedToken}\n`)
  process.exit(0)
} else if (kind === 'status') {
  value = scenario.no_account
    ? { hosts: {} }
    : { hosts: { [host]: [{ state: 'error', login: 'fork' }] } }
} else if (endpoint === '/user') {
  value = scenario.user
  if (scenario.rotate_token) writeFileSync(tokenFile, scenario.other_token)
  if (scenario.change_actor_on_second_user) {
    const count = `${required('CONTEXT_GH_STARTED')}-user-count`
    const number = existsSync(count)
      ? Number(readFileSync(count, 'utf8')) + 1
      : 1
    writeFileSync(count, String(number))
    if (number > 1)
      value = {
        ...scenario.user,
        id: scenario.user.id + 1,
        login: 'changed-actor',
      }
  }
} else if (endpoint === scenario.prefix) value = scenario.repository
else if (endpoint?.startsWith(`${scenario.prefix}/rules/branches/`)) {
  const page = endpoint.split('page=').at(-1) ?? ''
  value = scenario.pages?.[page] ?? scenario.rules
} else if (endpoint?.startsWith(`${scenario.prefix}/rulesets/`)) {
  const id = endpoint.split('/').at(-1) ?? ''
  value = scenario.rulesets[id]
  if (value === undefined) {
    process.stderr.write(scenario.secret)
    process.exit(1)
  }
} else {
  process.stderr.write(scenario.secret)
  process.exit(94)
}
process.stdout.write(`${JSON.stringify(value)}\n`)
