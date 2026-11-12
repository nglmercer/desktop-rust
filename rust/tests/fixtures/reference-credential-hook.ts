/** Probe the scoped credential helper inside a reference-transaction hook. */
import { spawnSync } from 'node:child_process'
import { readFileSync, writeFileSync } from 'node:fs'

const chunks: Buffer[] = []
for await (const chunk of process.stdin) chunks.push(Buffer.from(chunk))
if (
  process.argv[2] === 'prepared' &&
  Buffer.concat(chunks).includes('refs/heads/broker-reference-hook')
) {
  const scenario = JSON.parse(
    readFileSync(process.env.CONTEXT_SCENARIO ?? '', 'utf8')
  )
  const host = process.env.CREDENTIALS_HOOK_HOST ?? ''
  const request = `protocol=https\nhost=${host}\npath=fork/repo.git\n\n`
  const result = spawnSync('git', ['credential', 'fill'], {
    input: request,
    encoding: 'utf8',
    timeout: 35_000,
  })
  const fields = Object.fromEntries(
    result.stdout
      .split('\n')
      .filter(line => line.includes('='))
      .map(line => {
        const pos = line.indexOf('=')
        return [line.slice(0, pos), line.slice(pos + 1)]
      })
  )
  const ok =
    result.status === 0 && fields.password === scenario.users.AliceCase.token
  const parameter = spawnSync(
    'git',
    ['config', '--get', 'desktop.fixtureSecret'],
    { encoding: 'utf8', timeout: 5_000 }
  )
  const retained =
    parameter.status === 0 &&
    parameter.stdout.trimEnd() === scenario.fixture_secret
  writeFileSync(
    process.env.CREDENTIALS_HOOK_OUTPUT ?? '',
    JSON.stringify({
      ok,
      caller_parameter: retained,
      code: result.status,
      phase: process.argv[2],
    })
  )
  process.exit(ok && retained ? 0 : 91)
}
