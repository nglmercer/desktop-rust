/** Record which branch and remote a real Git LFS checkout child observes. */
import { spawnSync } from 'node:child_process'
import { appendFileSync } from 'node:fs'
const git = (...args: string[]): string | null => {
  const result = spawnSync(process.env.ACCOUNT_REAL_GIT ?? 'git', args, {
    encoding: 'utf8',
  })
  return result.status === 0 ? result.stdout.trimEnd() : null
}
const branch = git('symbolic-ref', '--quiet', '--short', 'HEAD')
const remote = branch ? git('config', '--get', `branch.${branch}.remote`) : null
appendFileSync(
  process.env.CREDENTIALS_LFS_ROUTE_LOG ?? '',
  `${JSON.stringify({ command: process.argv[2] ?? null, branch, remote, base_repository: 'fork/repo', head_repository: 'parent/repo', saved_source_remote: 'origin' })}\n`
)
const result = spawnSync(
  process.env.CREDENTIALS_REAL_LFS ?? 'git-lfs',
  process.argv.slice(2),
  { stdio: 'inherit' }
)
process.exit(result.status ?? 1)
