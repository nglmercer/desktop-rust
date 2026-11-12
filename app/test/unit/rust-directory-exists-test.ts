import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  transport: 'ws',
  directory: vi.fn(),
  stat: vi.fn(),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    directoryExists: mocks.directory,
  }),
}))
vi.mock('fs/promises', () => ({
  stat: mocks.stat,
  default: { stat: mocks.stat },
}))
import { directoryExists } from '../../src/lib/directory-exists'

describe('directory checks', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = 'ws'
  })
  it.each(['ws', 'wry'])(
    'uses the %s transport for the actual path',
    async transport => {
      mocks.transport = transport
      mocks.directory.mockResolvedValueOnce(true).mockResolvedValueOnce(false)
      await expect(directoryExists('/repo/.git')).resolves.toBe(true)
      await expect(directoryExists('/repo/file')).resolves.toBe(false)
      expect(mocks.directory).toHaveBeenCalledWith('/repo/.git')
      expect(mocks.stat).not.toHaveBeenCalled()
      await expect(directoryExists('')).resolves.toBe(false)
      expect(mocks.directory).toHaveBeenCalledTimes(2)
    }
  )
  it('propagates bridge failures and preserves the legacy filesystem fallback', async () => {
    const failure = new Error('disconnected')
    mocks.directory.mockRejectedValue(failure)
    await expect(directoryExists('/repo')).rejects.toBe(failure)
    mocks.transport = 'none'
    mocks.stat
      .mockResolvedValueOnce({ isDirectory: () => true })
      .mockRejectedValueOnce(new Error('missing'))
    await expect(directoryExists('/repo')).resolves.toBe(true)
    await expect(directoryExists('/missing')).resolves.toBe(false)
  })
})
