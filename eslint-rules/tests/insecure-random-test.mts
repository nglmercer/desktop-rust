// @vitest-environment node
import { spawnSync } from 'node:child_process'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

/** Exercise the installed Biome plugin, including its configuration and scope. */
function lint(source: string): { status: number | null; lines: number[] } {
  const root = resolve('.')
  const dir = mkdtempSync(join(root, 'app/test/biome-rule-'))
  try {
    const fixture = join(dir, 'input.ts')
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
        location: { start: { line: number } }
      }>
    }
    return {
      status: result.status,
      lines: report.diagnostics
        .filter(item => item.severity === 'error')
        .map(item => item.location.start.line),
    }
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

describe('Biome insecure-random rule', () => {
  it('preserves the legacy rule for direct, computed, optional and escaped identifiers', () => {
    const source = String.raw`Math.random();
(Math.random)();
(Math).random();
Math[random]();
crypto.pseudoRandomBytes();
other[pseudoRandomBytes]();
pseudoRandomBytes();
Math.r\u0061ndom();
\u004dath.random();
crypto.pseudoRandom\u0042ytes();
Math?.random?.();
Math /* object */ . /* member */ random();
Math[ /* member */ random /* end */ ]();
`
    expect(lint(source)).toEqual({
      status: 1,
      lines: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13],
    })
  })

  it('allows secure calls, non-call references and documented suppressions', () => {
    const source = `crypto.randomBytes()\nwindow.crypto.getRandomValues(new Uint8Array(4))\nconst fn = Math.random\nMath['random']()\n// biome-ignore lint/plugin: Non-security visual jitter.\nMath.random()\n`
    expect(lint(source)).toEqual({ status: 0, lines: [] })
  })
})
