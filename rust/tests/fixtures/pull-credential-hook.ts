/** Verify a Pull hook receives only its operation's exact-repository grant. */
import { spawnSync } from 'node:child_process'
import { readFileSync, writeFileSync } from 'node:fs'

const scenario = JSON.parse(
  readFileSync(process.env.CONTEXT_SCENARIO ?? '', 'utf8')
)
const account = process.env.CREDENTIALS_PULL_ACCOUNT ?? ''
const host = process.env.CREDENTIALS_PULL_HOST ?? ''
const token = scenario.users?.[account]?.token
if (typeof token !== 'string' || !host) {
  process.exit(81)
}

function credential(path: string) {
  return spawnSync(
    'git',
    ['-c', 'credential.useHttpPath=true', 'credential', 'fill'],
    {
      input: `protocol=https\nhost=${host}\npath=${path}\n\n`,
      encoding: 'utf8',
      timeout: 20_000,
    }
  )
}

const same = credential('fork/repo.git')
const other = credential('other/repo.git')
const prefix = credential('fork/repo-other.git')
const ownToken = same.stdout.split('\n').includes(`password=${token}`)
const secrets: unknown[] = [
  scenario.users?.AliceCase?.token,
  scenario.users?.BobCase?.token,
  scenario.environment?.token,
]
if (!secrets.every(value => typeof value === 'string' && value.length > 0)) {
  process.exit(83)
}
const denies = (result: typeof other) =>
  result.status !== 0 &&
  secrets.every(
    secret => typeof secret === 'string' && !result.stdout.includes(secret)
  )
const deniedOther = denies(other)
const deniedPrefix = denies(prefix)
writeFileSync(
  process.env.CREDENTIALS_PULL_PROBE_OUTPUT ?? '',
  JSON.stringify({ ownToken, deniedOther, deniedPrefix })
)
if (same.status !== 0 || !ownToken || !deniedOther || !deniedPrefix) {
  process.exit(82)
}
