// @vitest-environment node
import { spawnSync } from 'node:child_process'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

/** Exercise the actual Biome plugin, not a copy of its matching logic. */
function lint(source: string): {
  status: number | null
  lines: number[]
  messages: string[]
} {
  const root = resolve('.')
  const dir = mkdtempSync(join(root, 'app/test/biome-dispatcher-'))
  try {
    const fixture = join(dir, 'input.tsx')
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
    return {
      status: result.status,
      lines: report.diagnostics
        .filter(item => item.severity === 'error')
        .map(item => item.location.start.line),
      messages: report.diagnostics
        .filter(item => item.severity === 'error')
        .map(item => item.message),
    }
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

describe('Biome react-no-unbound-dispatcher-props rule', () => {
  it('detects the original exact JSX expression prefix in attributes and children', () => {
    const source = `<>
<X a={this.props.dispatcher.action} />
<X a={this.props.dispatcher.action()} />
<X>{this.props.dispatcher.action}</X>
<X a={this.props.dispatcher.action /* comment */} />
<X a={this.props.dispatcher.action as unknown} />
<X a={this.props.dispatcher.action ?? fallback} />
</>`
    expect(lint(source)).toMatchObject({ status: 1, lines: [2, 3, 4, 5, 6, 7] })
  })

  it('preserves the original whitespace, parenthesis, property and callback behavior', () => {
    const source = `<>
<X a={() => { this.props.dispatcher.action() }} />
<X a={ this.props.dispatcher.action} />
<X a={\nthis.props.dispatcher.action} />
<X a={(this.props.dispatcher.action)} />
<X a={this.props?.dispatcher.action} />
<X a={this.props['dispatcher'].action} />
<X a={other.props.dispatcher.action} />
<X {...this.props.dispatcher.action} />
</>`
    expect(lint(source)).toMatchObject({ status: 0, lines: [] })
  })

  it('matches legacy behavior around trivia, optional chaining and nested JSX', () => {
    const source = `<>
<X a={this.props.dispatcher.foo} b={this.props.dispatcher.bar} />
<X a={this.props.dispatcher.foo?.bar} />
<X a={this.props.dispatcher.foo
} />
<X a={/* comment */this.props.dispatcher.foo} />
<X a={this.props.dispatcher /* comment */ .foo} />
<X a={this.props.dispatcher.foo ? <Y b={this.props.dispatcher.bar} /> : null} />
{this.props.dispatcher.foo}
</>`
    expect(lint(source)).toMatchObject({
      status: 1,
      lines: [2, 2, 3, 4, 8, 8, 9],
    })
  })

  it('does not suppress the security rule for insecure randomness', () => {
    const source = `// biome-ignore lint/plugin/reactNoUnboundDispatcherProps: callback wrapped elsewhere
const x = <X a={this.props.dispatcher.foo(Math.random())} />`
    const result = lint(source)
    expect(result).toMatchObject({ status: 1, lines: [2] })
    expect(result.messages).toEqual([
      expect.stringContaining('Math.random produces insecure random numbers'),
    ])
  })

  it('does not hide the dispatcher rule when randomness is suppressed', () => {
    const source = `// biome-ignore lint/plugin/insecureRandom: test-only value
const x = <X a={this.props.dispatcher.foo(Math.random())} />`
    const result = lint(source)
    expect(result).toMatchObject({ status: 1, lines: [2] })
    expect(result.messages).toEqual([
      expect.stringContaining('Use of unbound dispatcher method'),
    ])
  })
})
