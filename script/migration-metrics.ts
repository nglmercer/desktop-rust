/**
 * Measures how much of the renderer still depends on the non-Rust code paths.
 * Run with `pnpm migration:status`; the numbers feed rust/ROADMAP.md.
 */
import { readdirSync, readFileSync, statSync } from 'node:fs'
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
  const vitestSuites = new Set<string>(
    JSON.parse(readFileSync(path.join(root, 'vitest-suites.json'), 'utf8')).map(
      (file: string) => path.resolve(root, file)
    )
  )
  const vitest = tests.filter(file => vitestSuites.has(path.resolve(file)))

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

if (require.main === module) {
  {
    const metrics = measure()
    const lines = [
      `Fallback checks (transportKind):   ${metrics.transportChecks} in ${metrics.transportCheckFiles} files`,
      `Legacy Git runner calls (lib/git): ${metrics.legacyGitCalls} in ${metrics.legacyGitCallFiles} files`,
      ...Object.entries(metrics.imports).map(
        ([name, files]) =>
          `Renderer files importing ${name.padEnd(14)} ${files}`
      ),
      `Unit tests on node:test:           ${metrics.nodeTestFiles} files`,
      `Unit tests on Vitest:              ${metrics.vitestTestFiles} files`,
      `Rust (crates, non-test):           ${metrics.rustLines} lines`,
    ]
    console.log(lines.join('\n'))
  }
}
