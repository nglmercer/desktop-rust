// @vitest-environment node
import { spawnSync } from 'node:child_process'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

const readonly = 'Property and state signatures should be read-only'
const array = 'Prop and State arrays should be read only (ReadOnlyArray)'

/** Run the installed Biome plugin against a real file. */
function lint(source: string, extension = 'tsx') {
  const root = resolve('.')
  const dir = mkdtempSync(join(root, 'app/test/biome-readonly-'))
  try {
    const fixture = join(dir, `input.${extension}`)
    writeFileSync(fixture, source)
    const result = spawnSync(
      process.execPath,
      [
        resolve('node_modules/@biomejs/biome/bin/biome'),
        'lint',
        '--config-path=biome.security.json',
        fixture,
        '--reporter=json',
      ],
      { cwd: root, encoding: 'utf8' }
    )
    if (result.error) {
      throw result.error
    }
    const report = JSON.parse(result.stdout) as {
      diagnostics: Array<{
        severity: string
        message: string
        location: { start: { line: number } }
      }>
    }
    return report.diagnostics
      .filter(item => item.severity === 'error')
      .map(item => [item.location.start.line, item.message])
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

describe('Biome react-readonly-props-and-state rule', () => {
  it('reports both mutable signatures and mutable arrays in Props and State', () => {
    const source = `interface IProps {
  name: string
  readonly items: Array<string>
  readonly values: string[]
  mutable: string[]
}
interface IState { value: string }
interface Other { mutable: string[] }
interface I\\u0050rops { value: string }`
    expect(lint(source)).toEqual([
      [2, readonly],
      [3, array],
      [4, array],
      [5, array],
      [5, readonly],
      [7, readonly],
      [9, readonly],
    ])
  })

  it('preserves legacy matching of spacing, comments and nested signatures', () => {
    const source = `interface IProps {
  readonly a:Array<string>
  readonly b:  Array<string>
  readonly c: Array<
string>
  readonly d: /* comment */ string[]
  readonly e: string[] /* comment */
  readonly f: ReadonlyArray<string>
  readonly g: (string[])
  [key: string]: unknown
  method(): string
  readonly nested: { mutable: string[] }
  'quoted'?: string[]
  [computed]: Array<number>
}`
    expect(lint(source)).toEqual([
      [3, array],
      [6, array],
      [7, array],
      [13, array],
      [13, readonly],
      [14, array],
      [14, readonly],
    ])
  })

  it('ignores TypeScript files, as the original rule did', () => {
    expect(lint('interface IState { mutable: string[] }', 'ts')).toEqual([])
  })

  it('allows suppressing this rule without hiding other security rules', () => {
    const source = `// biome-ignore lint/plugin/reactReadonlyPropsAndState: legacy interface
interface IProps { mutable: string }
const value = Math.random()`
    expect(lint(source)).toEqual([
      [
        3,
        expect.stringContaining('Math.random produces insecure random numbers'),
      ],
    ])
  })
})
