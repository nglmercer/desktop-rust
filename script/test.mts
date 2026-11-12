import { spawn } from 'node:child_process'
import { join, resolve } from 'node:path'
import { readdir, readFile, stat } from 'node:fs/promises'
import { parseEnv } from 'node:util'

function reporter(r: string): string[] {
  return ['--test-reporter', r, '--test-reporter-destination', 'stdout']
}

async function findTestFilesIn(paths: readonly string[]): Promise<string[]> {
  const files: string[] = []
  for (const path of paths) {
    const entry = await stat(path)
    if (entry.isFile()) {
      files.push(path)
      continue
    }

    for (const file of await readdir(path, { recursive: true }).then(x =>
      x
        .filter(f => /-test\.(ts|tsx|js|jsx|mts|mjs)$/.test(f))
        .map(f => join(path, f))
    )) {
      files.push(file)
    }
  }
  return files
}

const fileArgs = process.argv.slice(2).filter(a => !a.startsWith('--'))
const switchArgs = process.argv.slice(2).filter(a => a.startsWith('--'))

const projectRoot = join(import.meta.dirname, '..')
const files =
  fileArgs.length > 0
    ? await findTestFilesIn(fileArgs)
    : await findTestFilesIn([join(projectRoot, 'app', 'test', 'unit')])

// Suites move to Vitest one at a time; the rest still require Node's module
// mocking setup. Keep explicit routing until those suites have been migrated.
const suitePaths: unknown = JSON.parse(
  await readFile(join(projectRoot, 'vitest-suites.json'), 'utf8')
)
if (
  !Array.isArray(suitePaths) ||
  !suitePaths.every(path => typeof path === 'string')
) {
  throw new Error('vitest-suites.json must contain an array of paths')
}
const vitestSuites = new Set(suitePaths.map(file => resolve(projectRoot, file)))
const vitestFiles = files.filter(file => vitestSuites.has(resolve(file)))
const nodeFiles = files.filter(file => !vitestSuites.has(resolve(file)))
const vitestArgs: string[] = []
if (vitestFiles.length > 0) {
  for (const option of switchArgs) {
    if (option.startsWith('--test-name-pattern=')) {
      vitestArgs.push(
        '--testNamePattern',
        option.slice('--test-name-pattern='.length)
      )
    } else {
      throw new Error(
        `Unsupported option for migrated Vitest suites: ${option}`
      )
    }
  }
}

// I would _looooove_ to use the `--env-file` option, but it doesn't override
// existing environment variables and we need to override some of them.
const testEnv = parseEnv(await readFile(join(projectRoot, '.test.env'), 'utf8'))
Object.assign(process.env, testEnv)

const args = [
  '--disable-warning=ExperimentalWarning',
  '--experimental-test-module-mocks',
  // Allow CJS resolution to find ESM-only packages (e.g. @github/copilot-sdk)
  // whose "exports" only declare an "import" condition with no "require" fallback.
  '--conditions=import',
  ...['--import', 'tsx'],
  ...['--import', './app/test/globals.mts'],
  ...switchArgs,
  '--test',
  ...reporter('spec'),
  ...(process.env.GITHUB_ACTIONS ? reporter('node-test-github-reporter') : []),
  ...nodeFiles,
]

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

const nodeResult = nodeFiles.length > 0 ? await run(process.execPath, args) : 0
const vitestResult =
  vitestFiles.length > 0
    ? await run(process.execPath, [
        'node_modules/vitest/vitest.mjs',
        'run',
        '--config',
        'vitest.config.ts',
        ...vitestArgs,
        ...vitestFiles,
      ])
    : 0
process.exitCode = nodeResult || vitestResult
