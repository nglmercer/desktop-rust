import { directoryOf, isMainModule } from './esm-utils'
/**
 * Measures how much of the renderer still depends on the non-Rust code paths.
 * Run with `pnpm migration:status`; the numbers feed rust/ROADMAP.md.
 */
import { readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs'
import path from 'node:path'

const root = path.resolve(directoryOf(import.meta.url), '..')

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

// Comments and string contents are not command invocations. This small lexer
// keeps line/column structure stable enough for the conservative call-site
// count below without depending on a TypeScript AST implementation.
function stripCommentsAndStrings(source: string): string {
  let state: 'code' | 'single' | 'double' | 'template' | 'line' | 'block' =
    'code'
  let escaped = false
  let result = ''
  for (let index = 0; index < source.length; index++) {
    const character = source[index]
    const next = source[index + 1]
    if (state === 'code') {
      if (character === '/' && next === '/') {
        state = 'line'
        result += '  '
        index++
      } else if (character === '/' && next === '*') {
        state = 'block'
        result += '  '
        index++
      } else if (character === "'") {
        state = 'single'
        result += ' '
      } else if (character === '"') {
        state = 'double'
        result += ' '
      } else if (character === '`') {
        state = 'template'
        result += ' '
      } else {
        result += character
      }
      continue
    }
    if (state === 'line') {
      if (character === '\n') {
        state = 'code'
        result += character
      } else {
        result += ' '
      }
      continue
    }
    if (state === 'block') {
      if (character === '*' && next === '/') {
        state = 'code'
        result += '  '
        index++
      } else {
        result += character === '\n' ? character : ' '
      }
      continue
    }
    if (escaped) {
      escaped = false
    } else if (character === '\\') {
      escaped = true
    } else if (
      (state === 'single' && character === "'") ||
      (state === 'double' && character === '"') ||
      (state === 'template' && character === '`')
    ) {
      state = 'code'
    }
    result += character === '\n' ? character : ' '
  }
  return result
}

function countOccurrences(source: string, pattern: RegExp): number {
  return source.match(pattern)?.length ?? 0
}

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
  const coreRunner = path.join(srcRoot, 'lib', 'git', 'core.ts')
  const assistedRunner = path.join(
    srcRoot,
    'lib',
    'git',
    'assisted-commit',
    'git.ts'
  )
  const legacyGit = [...text].map(([file, source]) => {
    const stripped = stripCommentsAndStrings(source)
    // Every privateGit call is a renderer-specified Git command. The core
    // runner itself is transport infrastructure, and the two branches inside
    // privateGit are the shared transport for those call sites rather than
    // additional operations.
    const privateCalls = countOccurrences(
      stripped,
      /(?<!function\s)(?<![\w$.])privateGit\s*\(/g
    )
    const directCalls =
      file === coreRunner || file === assistedRunner
        ? 0
        : countOccurrences(stripped, /(?<!function\s)(?<![\w$.])git\s*\(/g)
    return privateCalls + directCalls
  })
  const imports: Record<string, number> = {}
  for (const [name, modules] of Object.entries({
    dugite: 'dugite|(?:\\.\\./)+web/dugite',
    fs: 'fs|fs/promises|node:fs|node:fs/promises|(?:\\.\\./)+web/fs|(?:\\.\\./)+web/fs-promises',
    child_process:
      'child_process|node:child_process|(?:\\.\\./)+web/child_process',
    os: 'os|node:os|(?:\\.\\./)+web/os',
    crypto: 'crypto|node:crypto|(?:\\.\\./)+web/crypto',
    keytar: 'keytar|(?:\\.\\./)+web/keytar',
    async_hooks: 'async_hooks|node:async_hooks|(?:\\.\\./)+web/async_hooks',
    net: 'net|node:net|(?:\\.\\./)+web/net',
    stream: 'stream|node:stream|(?:\\.\\./)+web/stream',
    stream_promises:
      'stream/promises|node:stream/promises|(?:\\.\\./)+web/stream-promises',
    path: 'path|node:path|(?:\\.\\./)+web/path',
    desktop_trampoline: 'desktop-trampoline|(?:\\.\\./)+web/desktop-trampoline',
    registry_js: 'registry-js|(?:\\.\\./)+web/registry-js',
    desktop_notifications:
      'desktop-notifications|(?:\\.\\./)+web/desktop-notifications',
    buffer: 'buffer|node:buffer',
    events: 'events|node:events',
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

  const fallbackCheck = /transportKind\s*(?:===|!==)/g
  const fallbackCheckFile = /transportKind\s*(?:===|!==)/
  return {
    // Count fallback decisions, not the backend transport descriptor itself.
    // The web-only host bridge is excluded above, so counting every identifier
    // would forever report the backend client's public `transportKind` getter
    // even after all operational fallback branches are gone.
    transportChecks: count(fallbackCheck),
    transportCheckFiles: filesMatching(fallbackCheckFile),
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
    `Renderer-driven Git command sites: ${metrics.legacyGitCalls} in ${metrics.legacyGitCallFiles} files`,
    ...Object.entries(metrics.imports).map(
      ([name, files]) => `Renderer files importing ${name.padEnd(14)} ${files}`
    ),
    `Unit tests on node:test:           ${metrics.nodeTestFiles} files`,
    `Unit tests on Vitest:              ${metrics.vitestTestFiles} files`,
    `Rust (crates, non-test):           ${metrics.rustLines} lines`,
  ]
  console.log(lines.join('\n'))
}

if (isMainModule(import.meta.url)) {
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
