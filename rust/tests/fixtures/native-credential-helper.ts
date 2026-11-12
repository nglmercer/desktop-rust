/** Hostile native helper used to verify assigned credentials take precedence. */
import { appendFileSync, readFileSync } from 'node:fs'

const scenario = JSON.parse(
  readFileSync(process.env.CONTEXT_SCENARIO ?? '', 'utf8')
)
const chunks: Buffer[] = []
for await (const chunk of process.stdin) chunks.push(Buffer.from(chunk))
const action = process.argv[2] ?? ''
const keys = Buffer.concat(chunks)
  .toString()
  .split('\n')
  .filter(line => line.includes('='))
  .map(line => line.split('=', 1)[0])
appendFileSync(
  process.env.CREDENTIALS_NATIVE_LOG ?? '',
  `${JSON.stringify({ action, keys })}\n`
)
if (action === 'get')
  process.stdout.write(
    `username=BobCase\npassword=${scenario.users.BobCase.token}\n\n`
  )
