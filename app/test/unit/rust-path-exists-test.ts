import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  transport: 'ws',
  exists: vi.fn(),
  access: vi.fn(),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({ transportKind: mocks.transport, pathExists: mocks.exists }),
}))
vi.mock('fs/promises', () => ({
  access: mocks.access,
  default: { access: mocks.access },
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
      expect(mocks.access).not.toHaveBeenCalled()
      await expect(pathExists('')).resolves.toBe(false)
      expect(mocks.exists).toHaveBeenCalledTimes(2)
    }
  )
  it('does not treat a disconnected bridge as a missing file', async () => {
    const failure = new Error('disconnected')
    mocks.exists.mockRejectedValue(failure)
    await expect(pathExists('/repo/README.md')).rejects.toBe(failure)
    expect(mocks.access).not.toHaveBeenCalled()
  })
  it('preserves the legacy filesystem fallback', async () => {
    mocks.transport = 'none'
    mocks.access
      .mockResolvedValueOnce(undefined)
      .mockRejectedValueOnce(new Error('missing'))
    await expect(pathExists('/repo/file')).resolves.toBe(true)
    await expect(pathExists('/missing')).resolves.toBe(false)
  })
})
