// @vitest-environment node
import { spawnSync } from 'node:child_process'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

/** Run the real pinned Biome plugin using the repository's security scope. */
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

describe('Biome no-loosely-typed-webcontents-ipc rule', () => {
  it('retains the legacy direct, optional, computed, escaped and parenthesized calls', () => {
    const source = String.raw`wc.send('channel');
wc?.send('channel');
wc.send?.('channel');
webContents.send('channel');
webContents?.send('channel');
webContents.send?.('channel');
window.webContents.send('channel');
window.webContents?.send('channel');
window?.webContents.send('channel');
window.webContents.send?.('channel');
(wc).send('channel');
(window.webContents.send)('channel');
wc[send]('channel');
window[webContents][send]('channel');
wc.s\u0065nd('channel');
window.webCont\u0065nts.send('channel');
`
    expect(lint(source)).toEqual({
      status: 1,
      lines: Array.from({ length: 16 }, (_, i) => i + 1),
    })
  })

  it('allows the same unrelated calls, strings, and non-call references', () => {
    const source = `ipcWebContents.send(webContents, 'channel')
other.send('channel')
other.wc.send('channel')
window.other.send('channel')
window['other'].send('channel')
webContents.reload()
webContents['send']('channel')
window['webContents'].send('channel')
const reference = wc.send
`
    expect(lint(source)).toEqual({ status: 0, lines: [] })
  })

  it('matches the legacy handling of TypeScript assertions and nested calls', () => {
    const source = `wc[send]('channel');
wc[(send)]('channel');
wc['send']('channel');
window[webContents].send('channel');
window['webContents'].send('channel');
(window.webContents as any).send('channel');
(wc as any).send('channel');
(window as any).webContents.send('channel');
window.webContents!.send('channel');
webContents!.send('channel');
wc.send<string>('channel');
wc.send.call(wc, 'channel');
new wc.send('channel');
`
    expect(lint(source)).toEqual({ status: 1, lines: [1, 2, 4, 8, 11] })
  })

  it('suppresses only the named rule, leaving other security diagnostics active', () => {
    const source = `// biome-ignore lint/plugin/noLooselyTypedWebContentsIpc: Typed helper implementation.
webContents.send(Math.random())
`
    expect(lint(source)).toEqual({ status: 1, lines: [2] })
  })

  it('does not hide IPC diagnostics when randomness is suppressed', () => {
    const source = `// biome-ignore lint/plugin/insecureRandom: Non-security visual jitter.
webContents.send(Math.random())
`
    expect(lint(source)).toEqual({ status: 1, lines: [2] })
  })
})
