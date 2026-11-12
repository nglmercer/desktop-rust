/** Local upload-pack transport for checking native SSH credential behavior. */
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { appendFileSync, readFileSync } from 'node:fs'
const scenario = JSON.parse(
  readFileSync(process.env.CONTEXT_SCENARIO ?? '', 'utf8')
)
const args = process.argv.slice(2)
const tokens: string[] = Object.values<{ token: string }>(scenario.users).map(
  user => user.token
)
appendFileSync(
  process.env.CREDENTIALS_SSH_LOG ?? '',
  `${JSON.stringify({ argv_clean: !args.some(arg => tokens.some(token => arg.includes(token))), alice_environment_clean: !Object.values(process.env).includes(scenario.users.AliceCase.token) })}\n`
)
assert.ok(args.at(-1)?.startsWith('git-upload-pack '))
const result = spawnSync(
  process.env.ACCOUNT_REAL_GIT ?? 'git',
  ['upload-pack', process.env.CREDENTIALS_SSH_REPO ?? ''],
  { stdio: 'inherit' }
)
process.exit(result.status ?? 1)
