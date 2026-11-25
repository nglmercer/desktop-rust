import { spawn } from 'node:child_process'
import { join, resolve } from 'node:path'
import { rmSync } from 'node:fs'
import { readFile } from 'node:fs/promises'
import { parseEnv } from 'node:util'
import { findTestFilesIn } from './test-discovery.mjs'
import {
  createCapableGitDirectory,
  filterByGitCapabilities,
} from './test-capabilities'

// pnpm forwards the conventional option separator, while npm removes it.
// It must not be passed through as a Node runtime flag.
const cliArgs = process.argv.slice(2).filter(a => a !== '--')
const fileArgs = cliArgs.filter(a => !a.startsWith('--'))
const switchArgs = cliArgs.filter(a => a.startsWith('--'))

const projectRoot = join(import.meta.dirname, '..')
const discovered =
  fileArgs.length > 0
    ? await findTestFilesIn(fileArgs)
    : await findTestFilesIn([join(projectRoot, 'app', 'test', 'unit')])

// Prefer a capable Git over skipping: when the configured one lacks behavior
// the suites assert, run the whole run against the Git on PATH if it has it.
const capableGit = createCapableGitDirectory()
if (capableGit !== undefined) {
  console.warn(
    `Using ${capableGit.git} for tests through a temporary dugite-compatible Git layout`
  )
  process.env.LOCAL_GIT_DIRECTORY = capableGit.directory
  process.on('exit', () => {
    rmSync(capableGit.directory, { recursive: true, force: true })
  })
}

// Some suites need Git behavior the installed Git may not have. Skip them
// loudly rather than reporting failures that no code change can fix.
const { files, skipped } = filterByGitCapabilities(discovered, file =>
  resolve(projectRoot, file)
)
for (const { file, reason } of skipped) {
  console.warn(`SKIPPED ${file}: ${reason}`)
}

const vitestArgs: string[] = []
for (const option of switchArgs) {
  if (option.startsWith('--test-name-pattern=')) {
    vitestArgs.push(
      '--testNamePattern',
      option.slice('--test-name-pattern='.length)
    )
  } else {
    throw new Error(`Unsupported Vitest option: ${option}`)
  }
}

// I would _looooove_ to use the `--env-file` option, but it doesn't override
// existing environment variables and we need to override some of them.
const testEnv = parseEnv(await readFile(join(projectRoot, '.test.env'), 'utf8'))
Object.assign(process.env, testEnv)

function run(command: string, commandArgs: readonly string[]): Promise<number> {
  return new Promise((resolveRun, rejectRun) => {
    const child = spawn(command, commandArgs, {
      stdio: 'inherit',
      cwd: projectRoot,
    })
    child.on('error', rejectRun)
    child.on('exit', (code, signal) => resolveRun(code ?? (signal ? 1 : 0)))
  })
}

// The Vitest suites run against the real Rust backend by default (see
// rust/ROADMAP.md, R1/R2): build the server they start, unless the caller opted
// out with DESKTOP_TEST_RUST_BACKEND=0 or this platform is not validated yet.
if (files.length > 0 && process.env.DESKTOP_TEST_RUST_BACKEND !== '0') {
  if (process.platform === 'linux') {
    const built = await run('cargo', [
      'build',
      '--manifest-path',
      join(projectRoot, 'Cargo.toml'),
      '-p',
      'desktop-cli',
    ]).catch(() => 1)
    if (built === 0) {
      process.env.DESKTOP_TEST_RUST_BACKEND = '1'
    } else {
      console.warn(
        'Could not build the Rust backend (desktop-cli); running Vitest without it. Set DESKTOP_TEST_RUST_BACKEND=0 to silence this.'
      )
    }
  }
}

process.exitCode =
  files.length > 0
    ? await run(process.execPath, [
        'node_modules/vitest/vitest.mjs',
        'run',
        '--config',
        'vitest.config.ts',
        ...vitestArgs,
        ...files,
      ])
    : 0
