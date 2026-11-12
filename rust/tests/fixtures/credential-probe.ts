/** Pre-push probe for the operation-scoped Git credential helper. */
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { readFileSync, writeFileSync } from 'node:fs'

interface Probe {
  valid: string[]
  invalid: [string, string][]
  rawInvalid: [string, string][]
  child: string
  output: string
  scenario: string
  host: string
}
const control: Probe = JSON.parse(
  readFileSync(process.env.CREDENTIALS_PROBE_CONTROL ?? '', 'utf8')
)
const scenario = JSON.parse(readFileSync(control.scenario, 'utf8'))
const secrets = [
  ...Object.values<{ token: string }>(scenario.users).map(user => user.token),
  scenario.environment.token,
]
const expected: string = scenario.users.AliceCase.token
const rows: Array<{ label: string; ok: boolean; code: number | null }> = []
function run(payload: string, args: string[], cwd?: string) {
  return spawnSync(args[0], args.slice(1), {
    input: payload,
    cwd,
    encoding: 'utf8',
    timeout: 12_000,
  })
}
function record(
  label: string,
  result: ReturnType<typeof run>,
  valid = false,
  raw = false
): void {
  const lines = result.stdout.split(/\r?\n/)
  const actual = Object.fromEntries(
    lines
      .filter(line => line.includes('='))
      .map(line => {
        const index = line.indexOf('=')
        return [line.slice(0, index), line.slice(index + 1)]
      })
  )
  const ok = valid
    ? result.status === 0 && actual.password === expected
    : !secrets.some(secret => result.stdout.includes(secret)) &&
      (raw ? lines.includes('quit=true') : result.status !== 0)
  rows.push({ label, ok, code: result.status })
}
const args = [
  'git',
  '-c',
  `credential.helper=${process.env.CREDENTIALS_HOSTILE_HELPER}`,
  'credential',
  'fill',
]
for (const [index, payload] of control.valid.entries())
  record(`valid-${index}`, run(payload, args), true)
for (const [label, payload] of control.invalid)
  record(label, run(payload, args))
record('submodule-cannot-borrow', run(control.valid[0], args, control.child))
const helpers = spawnSync(
  'git',
  ['config', '--get-all', `credential.https://${control.host}.helper`],
  { encoding: 'utf8', timeout: 12_000 }
)
assert.equal(helpers.status, 0)
const helper = helpers.stdout
  .split('\n')
  .reverse()
  .find(value => value && value !== process.env.CREDENTIALS_HOSTILE_HELPER)
assert.ok(helper)
assert.ok(helper.startsWith('!'))
for (const [label, payload] of control.rawInvalid)
  record(
    label,
    run(payload, ['sh', '-c', `${helper.slice(1)} get`]),
    false,
    true
  )
for (const action of ['store', 'erase']) {
  const payload = `${control.valid[0].trimEnd()}\nusername=AliceCase\npassword=${expected}\n\n`
  const result = run(payload, ['sh', '-c', `${helper.slice(1)} ${action}`])
  rows.push({
    label: `${action}-read-only`,
    ok: result.status === 0 && !result.stdout,
    code: result.status,
  })
}
writeFileSync(control.output, JSON.stringify(rows))
process.exit(rows.every(row => row.ok) ? 0 : 91)
