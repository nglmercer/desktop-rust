import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { appendFileSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { tailByLine } from '../../src/lib/file-system'
import {
  installNodeHostBridge,
  uninstallNodeHostBridge,
} from '../helpers/node-host-bridge'

async function waitFor(assertion: () => void) {
  const deadline = Date.now() + 4000
  let lastError: unknown
  while (Date.now() < deadline) {
    try {
      assertion()
      return
    } catch (error) {
      lastError = error
      await new Promise(resolve => setTimeout(resolve, 50))
    }
  }
  throw lastError
}

describe('tailByLine through the host-backed filesystem', () => {
  let directory: string

  beforeEach(() => {
    installNodeHostBridge()
    directory = mkdtempSync(join(tmpdir(), 'desktop-tailer-web-fs-'))
  })

  afterEach(() => {
    uninstallNodeHostBridge()
    rmSync(directory, { recursive: true, force: true })
  })

  it('reports appended lines using host stat, watch, and ranged reads', async () => {
    const path = join(directory, 'progress')
    writeFileSync(path, '')
    const lines: string[] = []
    const disposable = tailByLine(path, line => lines.push(line))
    try {
      await new Promise(resolve => setTimeout(resolve, 1000))
      appendFileSync(path, 'first\n')
      await waitFor(() => expect(lines).toEqual(['first']))

      appendFileSync(path, 'second\n')
      await waitFor(() => expect(lines).toEqual(['first', 'second']))
    } finally {
      disposable.dispose()
    }
  })
})
