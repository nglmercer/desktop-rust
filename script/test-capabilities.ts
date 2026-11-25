import { execFileSync, spawnSync } from 'node:child_process'
import {
  copyFileSync,
  linkSync,
  mkdirSync,
  mkdtempSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'

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

/**
 * Describe the Git on PATH in the layout dugite expects (`bin/git`,
 * `libexec/git-core`, `share`). This is required because dependencies are
 * installed with install scripts disabled, so dugite's embedded Git download is
 * not present. When a caller explicitly configured LOCAL_GIT_DIRECTORY, keep it
 * unless it lacks a capability that the system Git can provide.
 */
export function createCapableGitDirectory(
  env: NodeJS.ProcessEnv = process.env
): { directory: string; git: string } | undefined {
  const configuredDirectory = env.LOCAL_GIT_DIRECTORY
  if (
    configuredDirectory !== undefined &&
    configuredDirectory !== '' &&
    gitHasCapability('remote-url-reset', env)
  ) {
    return undefined
  }
  const pathEnv = {
    ...process.env,
    ...env,
    LOCAL_GIT_DIRECTORY: '',
  }
  let git: string
  let execPath: string
  try {
    const locator = process.platform === 'win32' ? 'where' : 'which'
    const located = execFileSync(locator, ['git'], {
      encoding: 'utf8',
      env: pathEnv,
    })
      .split(/\r?\n/)
      .map(line => line.trim())
      .filter(line => line.length > 0)[0]
    if (located === undefined) {
      return undefined
    }
    git = realpathSync(located)
    execPath = realpathSync(
      execFileSync(git, ['--exec-path'], {
        encoding: 'utf8',
        env: pathEnv,
      }).trim()
    )
  } catch {
    return undefined
  }
  const directory = mkdtempSync(join(tmpdir(), 'desktop-test-git-'))
  if (process.platform === 'win32') {
    // Dugite expects a Git-for-Windows layout whose runtime subdirectory name
    // depends on the Node architecture. Link the system Git's actual runtime
    // directory into that layout without copying the whole installation.
    const runtime =
      process.arch === 'x64'
        ? 'ucrt64'
        : process.arch === 'arm64'
          ? 'clangarm64'
          : 'mingw32'
    const runtimeRoot = dirname(dirname(execPath))
    const runtimeLink = join(directory, runtime)
    mkdirSync(join(directory, 'cmd'))
    mkdirSync(join(runtimeLink, 'libexec'), { recursive: true })
    mkdirSync(join(runtimeLink, 'usr'), { recursive: true })
    try {
      linkSync(git, join(directory, 'cmd', 'git.exe'))
    } catch {
      copyFileSync(git, join(directory, 'cmd', 'git.exe'))
    }
    symlinkSync(execPath, join(runtimeLink, 'libexec', 'git-core'), 'junction')
    symlinkSync(join(runtimeRoot, 'bin'), join(runtimeLink, 'bin'), 'junction')
    symlinkSync(
      join(runtimeRoot, 'usr', 'bin'),
      join(runtimeLink, 'usr', 'bin'),
      'junction'
    )
    return { directory, git }
  }
  const prefix = dirname(dirname(git))
  mkdirSync(join(directory, 'bin'))
  mkdirSync(join(directory, 'libexec'))
  symlinkSync(git, join(directory, 'bin', 'git'))
  symlinkSync(execPath, join(directory, 'libexec', 'git-core'))
  symlinkSync(join(prefix, 'share'), join(directory, 'share'))
  // Even when this Git lacks optional capabilities, returning the layout is
  // necessary: it gives dugite a real executable and the capability filter can
  // then skip only the suites that need behavior this Git does not provide.
  return { directory, git }
}

/**
 * Newer Git reports a missing alternate object directory as an error from
 * `count-objects -v`, which the assisted-commit status load treats as fatal
 * before any commit is made. The suites that expect that case to surface only
 * at push time are marked `todo` on such a Git.
 */
export function gitReportsMissingAlternateAsError(
  env: NodeJS.ProcessEnv = process.env
): boolean {
  const git = testGitExecutable(env)
  const directory = mkdtempSync(join(tmpdir(), 'desktop-git-probe-'))
  try {
    const run = (args: string[]) =>
      spawnSync(git, args, {
        cwd: directory,
        encoding: 'utf8',
        env: { ...env, GIT_CONFIG_GLOBAL: '/dev/null', LC_ALL: 'C' },
      })
    run(['init', '-q', '.'])
    mkdirSync(join(directory, '.git', 'objects', 'info'), { recursive: true })
    writeFileSync(
      join(directory, '.git', 'objects', 'info', 'alternates'),
      `${join(directory, 'missing-objects')}\n`
    )
    return /^error: object directory .* does not exist/m.test(
      run(['count-objects', '-v']).stderr
    )
  } catch {
    return false
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
}
