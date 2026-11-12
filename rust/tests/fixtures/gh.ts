import { appendFileSync, readFileSync } from 'node:fs'

const args = process.argv.slice(2)
const mode = process.env.GH_TEST_MODE ?? 'ok'
if (mode === 'reject_input') {
  console.error('authentication required')
  process.exit(4)
}
const stdin = readFileSync(0, 'utf8')
if (process.env.GH_PROMPT_DISABLED !== '1' || !process.env.GH_TEST_LOG) {
  process.exit(90)
}
appendFileSync(process.env.GH_TEST_LOG, `${JSON.stringify({ args, stdin })}\n`)
if (mode === 'auth_required') {
  console.error('authentication required')
  process.exit(4)
}
if (mode === 'invalid') {
  console.log('not JSON')
} else if (args[0] === 'auth' && args[1] === 'status') {
  console.log(
    JSON.stringify({
      hosts: {
        'github.com': [
          {
            host: 'github.com',
            login: 'test-user',
            active: true,
            state: 'success',
            oauthToken: 'DO-NOT-EXPOSE',
            token: 'DO-NOT-EXPOSE',
          },
        ],
      },
    })
  )
} else if (args.includes('--json')) {
  console.log(
    JSON.stringify(
      ['checks', 'list'].includes(args[1])
        ? [{ name: 'test', state: 'PENDING' }]
        : { number: 7, title: 'fixture' }
    )
  )
} else {
  console.log('https://github.com/owner/repo/pull/7')
}
if (mode === 'pending') process.exit(8)
