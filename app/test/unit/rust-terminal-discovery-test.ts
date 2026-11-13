import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  terminals: vi.fn(),
}))

vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: 'ws',
    availableRustTerminals: mocks.terminals,
  }),
}))

import {
  findShellOrDefault,
  getAvailableShells,
} from '../../src/lib/shells/shared'
import { Shell } from '../../src/lib/shells/linux'

describe('Rust terminal discovery', () => {
  beforeEach(() => mocks.terminals.mockReset())

  it('uses host-discovered terminals and ignores unknown names', async () => {
    mocks.terminals.mockResolvedValue([
      { name: Shell.Konsole, path: '/usr/bin/konsole' },
      { name: 'Unknown terminal', path: '/usr/bin/unknown' },
    ])
    await expect(getAvailableShells()).resolves.toEqual([
      { shell: Shell.Konsole, path: '/usr/bin/konsole' },
    ])
    await expect(findShellOrDefault(Shell.Gnome)).resolves.toEqual({
      shell: Shell.Konsole,
      path: '/usr/bin/konsole',
    })
  })

  it('reports when no supported host terminal is available', async () => {
    mocks.terminals.mockResolvedValue([])
    await expect(findShellOrDefault(Shell.Gnome)).rejects.toThrow(
      'No supported terminal is available'
    )
  })
})
