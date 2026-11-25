import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  transport: 'ws',
  exists: vi.fn(),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({ transportKind: mocks.transport, pathExists: mocks.exists }),
}))
import { pathExists } from '../../src/lib/path-exists'

describe('path checks', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = 'ws'
  })
  it.each(['ws', 'wry'])(
    'uses the %s transport for files and directories',
    async transport => {
      mocks.transport = transport
      mocks.exists.mockResolvedValueOnce(true).mockResolvedValueOnce(false)
      await expect(pathExists('/repo/README.md')).resolves.toBe(true)
      await expect(pathExists('/repo/missing')).resolves.toBe(false)
      expect(mocks.exists).toHaveBeenCalledWith('/repo/README.md')
      await expect(pathExists('')).resolves.toBe(false)
      expect(mocks.exists).toHaveBeenCalledTimes(2)
    }
  )
  it('does not treat a disconnected bridge as a missing file', async () => {
    const failure = new Error('disconnected')
    mocks.exists.mockRejectedValue(failure)
    await expect(pathExists('/repo/README.md')).rejects.toBe(failure)
  })
})
