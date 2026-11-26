// @vitest-environment node
import { mkdtempSync, rmSync, writeFileSync, mkdirSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import {
  installNodeHostBridge,
  uninstallNodeHostBridge,
} from '../helpers/node-host-bridge'
import { which } from '../../src/lib/executable-path'

describe.skipIf(process.platform === 'win32')(
  'executable path resolution',
  () => {
    let root = ''

    beforeEach(() => {
      installNodeHostBridge()
      root = mkdtempSync(join(tmpdir(), 'desktop-which-'))
    })

    afterEach(() => {
      uninstallNodeHostBridge()
      rmSync(root, { recursive: true, force: true })
      root = ''
    })

    it('returns the first executable in a custom PATH', async () => {
      const first = join(root, 'first')
      const second = join(root, 'second')
      mkdirSync(first)
      mkdirSync(second)
      writeFileSync(join(first, 'tool'), 'first\n', { mode: 0o644 })
      writeFileSync(join(second, 'tool'), 'second\n', { mode: 0o755 })

      await expect(which('tool', { path: `${first}:${second}` })).resolves.toBe(
        join(second, 'tool')
      )
    })

    it('requires execute permission for regular files', async () => {
      const path = join(root, 'tool')
      writeFileSync(path, 'not executable\n', { mode: 0o644 })
      await expect(which('tool', { path: root })).rejects.toMatchObject({
        code: 'ENOENT',
      })
      await expect(
        which('tool', { path: root, nothrow: true })
      ).resolves.toBeNull()
    })

    it('resolves an explicit executable path', async () => {
      const path = join(root, 'tool')
      writeFileSync(path, 'executable\n', { mode: 0o755 })
      await expect(which(path, { nothrow: true })).resolves.toBe(path)
    })
  }
)
