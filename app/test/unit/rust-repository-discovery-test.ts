import { beforeEach, describe, expect, it, vi } from 'vitest'
const mocks = vi.hoisted(() => ({
  transport: 'ws',
  type: vi.fn(),
  trust: vi.fn(),
  directory: vi.fn(),
  git: vi.fn(),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    repositoryType: mocks.type,
    trustRepository: mocks.trust,
  }),
}))
vi.mock('../../src/lib/directory-exists', () => ({
  directoryExists: mocks.directory,
}))
vi.mock('../../src/lib/git/core', () => ({ git: mocks.git }))
import { getRepositoryType } from '../../src/lib/git/rev-parse'
import { addSafeDirectory } from '../../src/lib/git/config'

describe('repository discovery routing', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = 'ws'
  })
  it.each(['ws', 'wry'])(
    'uses %s for repository discovery',
    async transport => {
      mocks.transport = transport
      mocks.type.mockResolvedValue({ kind: 'unsafe', path: '/repo' })
      await expect(getRepositoryType('/repo/nested')).resolves.toEqual({
        kind: 'unsafe',
        path: '/repo',
      })
      expect(mocks.type).toHaveBeenCalledWith('/repo/nested')
      expect(mocks.directory).not.toHaveBeenCalled()
      expect(mocks.git).not.toHaveBeenCalled()
      await expect(getRepositoryType('')).resolves.toEqual({ kind: 'missing' })
    }
  )
  it('propagates transport failures without a shell fallback', async () => {
    const failure = new Error('disconnected')
    mocks.type.mockRejectedValue(failure)
    await expect(getRepositoryType('/repo')).rejects.toBe(failure)
    expect(mocks.git).not.toHaveBeenCalled()
  })
  it.each(['ws', 'wry'])(
    'routes explicit trust through %s',
    async transport => {
      mocks.transport = transport
      mocks.trust.mockResolvedValue(undefined)
      await addSafeDirectory('/repo')
      expect(mocks.trust).toHaveBeenCalledWith('/repo')
      expect(mocks.git).not.toHaveBeenCalled()
      await expect(addSafeDirectory('relative')).rejects.toThrow(
        'absolute path'
      )
      expect(mocks.trust).toHaveBeenCalledOnce()
    }
  )
})
