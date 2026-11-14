import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

/** Suites that assert behavior only some Git installations provide. */
export interface IGitCapabilityRequirement {
  readonly capability: 'remote-url-reset' | 'git-lfs'
  readonly description: string
  readonly files: ReadonlyArray<string>
}

export const gitCapabilityRequirements: ReadonlyArray<IGitCapabilityRequirement> =
  [
    {
      capability: 'remote-url-reset',
      description:
        'an empty `remote.<name>.url=` in GIT_CONFIG_PARAMETERS must reset the URL list for `git remote get-url` (the assisted-push destination guard pins the URL this way; Git 2.45 appends an empty entry instead)',
      files: [
        'app/test/unit/git/assisted-commit-push-destination-test.ts',
        'app/test/unit/stores/assisted-commit-push-boundary-test.ts',
        'app/test/unit/stores/assisted-commit-push-hooks-test.ts',
        'app/test/unit/stores/assisted-commit-push-lifecycle-test.ts',
        'app/test/unit/stores/assisted-commit-push-test.ts',
      ],
    },
    {
      capability: 'git-lfs',
      description: '`git lfs` must be installed for the tracked-path checks',
      files: ['app/test/unit/git/lfs-test.ts'],
    },
  ]

/** The Git executable the application tests run, as dugite would choose it. */
export function testGitExecutable(
  env: NodeJS.ProcessEnv = process.env
): string {
  const directory = env.LOCAL_GIT_DIRECTORY
  return directory === undefined || directory === ''
    ? 'git'
    : join(directory, 'bin', 'git')
}

/** Probe whether the test Git provides a capability. */
export function gitHasCapability(
  capability: IGitCapabilityRequirement['capability'],
  env: NodeJS.ProcessEnv = process.env
): boolean {
  const git = testGitExecutable(env)
  try {
    if (capability === 'git-lfs') {
      execFileSync(git, ['lfs', 'version'], { stdio: 'ignore', env })
      return true
    }
    const directory = mkdtempSync(join(tmpdir(), 'desktop-git-probe-'))
    try {
      const run = (args: string[], extra: NodeJS.ProcessEnv = {}) =>
        execFileSync(git, args, {
          cwd: directory,
          encoding: 'utf8',
          env: { ...env, ...extra, GIT_CONFIG_GLOBAL: '/dev/null' },
        })
      run(['init', '-q', '.'])
      run(['remote', 'add', 'origin', 'https://example.invalid/a.git'])
      const output = run(['remote', 'get-url', '--push', '--all', 'origin'], {
        GIT_CONFIG_PARAMETERS:
          "'remote.origin.url=' 'remote.origin.url=https://example.invalid/a.git'",
      })
      return output === 'https://example.invalid/a.git\n'
    } finally {
      rmSync(directory, { recursive: true, force: true })
    }
  } catch {
    return false
  }
}

/** Remove suites whose required Git capabilities are unavailable. */
export function filterByGitCapabilities(
  files: ReadonlyArray<string>,
  resolvePath: (file: string) => string,
  env: NodeJS.ProcessEnv = process.env
): { files: string[]; skipped: Array<{ file: string; reason: string }> } {
  const skipped: Array<{ file: string; reason: string }> = []
  const unavailable = new Map<string, string>()
  for (const requirement of gitCapabilityRequirements) {
    if (
      !files.some(file =>
        requirement.files.some(r => resolvePath(r) === resolvePath(file))
      )
    ) {
      continue
    }
    if (!gitHasCapability(requirement.capability, env)) {
      for (const file of requirement.files) {
        unavailable.set(
          resolvePath(file),
          `${testGitExecutable(env)} lacks ${requirement.capability}: ${
            requirement.description
          }`
        )
      }
    }
  }
  const kept = files.filter(file => {
    const reason = unavailable.get(resolvePath(file))
    if (reason === undefined) {
      return true
    }
    skipped.push({ file, reason })
    return false
  })
  return { files: kept, skipped }
}
