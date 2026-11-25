/**
 * Measures how much of the renderer still depends on the non-Rust code paths.
 * Run with `pnpm migration:status`; the numbers feed rust/ROADMAP.md.
 */
import { readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs'
import path from 'node:path'

const root = path.resolve(__dirname, '..')

function walk(directory: string, accept: (file: string) => boolean): string[] {
  const result: string[] = []
  for (const entry of readdirSync(directory)) {
    const file = path.join(directory, entry)
    if (entry === 'node_modules' || entry === 'target') continue
    if (statSync(file).isDirectory()) {
      result.push(...walk(file, accept))
    } else if (accept(file)) {
      result.push(file)
    }
  }
  return result
}

const isSource = (file: string) =>
  /\.(ts|tsx)$/.test(file) && !file.endsWith('.d.ts')
const srcRoot = path.join(root, 'app', 'src')
const sources = walk(srcRoot, isSource).filter(
  file => !file.startsWith(path.join(srcRoot, 'web') + path.sep)
)
const text = new Map(sources.map(file => [file, readFileSync(file, 'utf8')]))
const count = (pattern: RegExp) =>
  [...text.values()].reduce(
    (sum, source) => sum + (source.match(pattern)?.length ?? 0),
    0
  )
const filesMatching = (pattern: RegExp) =>
  [...text.values()].filter(source => pattern.test(source)).length
const importOf = (modules: string) =>
  new RegExp(`(?:from|require\\()\\s*['"](?:${modules})['"]`)

export interface IMetrics {
  readonly transportChecks: number
  readonly transportCheckFiles: number
  readonly legacyGitCalls: number
  readonly legacyGitCallFiles: number
  readonly imports: Readonly<Record<string, number>>
  readonly nodeTestFiles: number
  readonly vitestTestFiles: number
  readonly rustLines: number
}

export function measure(): IMetrics {
  const gitSources = [...text].filter(([file]) =>
    file.startsWith(path.join(srcRoot, 'lib', 'git') + path.sep)
  )
  const legacyGit = gitSources.map(
    ([, source]) => source.match(/await git\(/g)?.length ?? 0
  )
  const imports: Record<string, number> = {}
  for (const [name, modules] of Object.entries({
    dugite: 'dugite',
    fs: 'fs|fs/promises|node:fs|node:fs/promises',
    child_process: 'child_process|node:child_process',
    os: 'os|node:os',
    crypto: 'crypto|node:crypto',
    keytar: 'keytar',
  })) {
    imports[name] = filesMatching(importOf(modules))
  }

  const tests = walk(path.join(root, 'app', 'test', 'unit'), file =>
    /(?:-test|\.test)\.(ts|tsx|js|jsx|mts|mjs)$/.test(file)
  )
  // Vitest is the only JavaScript test runner. The compatibility counts remain
  // in the metrics shape so the baseline can continue guarding the migration.
  const vitest = tests

  const rustLines = walk(path.join(root, 'rust', 'crates'), file =>
    file.endsWith('.rs')
  )
    .filter(file => !file.includes(`${path.sep}tests${path.sep}`))
    .reduce(
      (sum, file) => sum + readFileSync(file, 'utf8').split('\n').length,
      0
    )

  return {
    transportChecks: count(/transportKind/g),
    transportCheckFiles: filesMatching(/transportKind/),
    legacyGitCalls: legacyGit.reduce((a, b) => a + b, 0),
    legacyGitCallFiles: legacyGit.filter(n => n > 0).length,
    imports,
    nodeTestFiles: tests.length - vitest.length,
    vitestTestFiles: vitest.length,
    rustLines,
  }
}

/** The numbers the migration is trying to drive to zero. Lower is better. */
export function targets(metrics: IMetrics): Readonly<Record<string, number>> {
  return {
    transportChecks: metrics.transportChecks,
    legacyGitCalls: metrics.legacyGitCalls,
    nodeTestFiles: metrics.nodeTestFiles,
    ...Object.fromEntries(
      Object.entries(metrics.imports).map(([name, files]) => [
        `imports.${name}`,
        files,
      ])
    ),
  }
}

const baselinePath = path.join(root, 'rust', 'migration-baseline.json')

/**
 * Compare with the committed baseline. A number that went up fails; one that
 * went down asks for the baseline to be lowered so progress cannot be lost.
 */
export function compareWithBaseline(
  current: Readonly<Record<string, number>>,
  baseline: Readonly<Record<string, number>>
): { regressions: string[]; improvements: string[] } {
  const regressions: string[] = []
  const improvements: string[] = []
  for (const [name, value] of Object.entries(current)) {
    const limit = baseline[name]
    if (limit === undefined) {
      regressions.push(`${name}: ${value} has no baseline entry`)
    } else if (value > limit) {
      regressions.push(`${name}: ${value} is above the baseline ${limit}`)
    } else if (value < limit) {
      improvements.push(`${name}: ${value} is below the baseline ${limit}`)
    }
  }
  return { regressions, improvements }
}

function report(metrics: IMetrics) {
  const lines = [
    `Fallback checks (transportKind):   ${metrics.transportChecks} in ${metrics.transportCheckFiles} files`,
    `Legacy Git runner calls (lib/git): ${metrics.legacyGitCalls} in ${metrics.legacyGitCallFiles} files`,
    ...Object.entries(metrics.imports).map(
      ([name, files]) => `Renderer files importing ${name.padEnd(14)} ${files}`
    ),
    `Unit tests on node:test:           ${metrics.nodeTestFiles} files`,
    `Unit tests on Vitest:              ${metrics.vitestTestFiles} files`,
    `Rust (crates, non-test):           ${metrics.rustLines} lines`,
  ]
  console.log(lines.join('\n'))
}

if (require.main === module) {
  const metrics = measure()
  const mode = process.argv[2]
  if (mode === '--update') {
    writeFileSync(
      baselinePath,
      `${JSON.stringify(targets(metrics), null, 2)}\n`
    )
    console.log(`Baseline written to ${path.relative(root, baselinePath)}`)
  } else if (mode === '--check') {
    const baseline = JSON.parse(readFileSync(baselinePath, 'utf8'))
    const { regressions, improvements } = compareWithBaseline(
      targets(metrics),
      baseline
    )
    for (const line of regressions) console.error(`REGRESSION ${line}`)
    for (const line of improvements) {
      console.log(`IMPROVED   ${line} (run pnpm migration:baseline)`)
    }
    if (regressions.length > 0) {
      process.exit(1)
    }
    console.log('Migration metrics are within the baseline.')
  } else {
    report(metrics)
  }
}
