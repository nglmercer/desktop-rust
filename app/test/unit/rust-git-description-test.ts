import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  description: vi.fn(),
  setDescription: vi.fn(),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    repositoryGitDescription: mocks.description,
    setRepositoryGitDescription: mocks.setDescription,
  }),
}))
import {
  getGitDescription,
  writeGitDescription,
} from '../../src/lib/git/description'

describe('repository Git description', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it.each(['ws', 'wry'])('uses the scoped Rust reader on %s', async () => {
    mocks.description.mockResolvedValueOnce('  project\n')
    await expect(getGitDescription('/repo')).resolves.toBe('  project\n')
    expect(mocks.description).toHaveBeenCalledWith('/repo')

    // The default-description normalization is a backend concern: whatever
    // the Rust reader returns is passed through unchanged.
    mocks.description.mockResolvedValueOnce('')
    await expect(getGitDescription('/repo')).resolves.toBe('')

    mocks.description.mockRejectedValueOnce(new Error('disconnected'))
    await expect(getGitDescription('/repo')).rejects.toThrow('disconnected')
  })

  it.each(['ws', 'wry'])(
    'writes via the Rust repository operation on %s',
    async () => {
      mocks.setDescription.mockResolvedValueOnce(undefined)
      await writeGitDescription('/repo', 'project 🦀\n')
      expect(mocks.setDescription).toHaveBeenCalledWith('/repo', 'project 🦀\n')
      mocks.setDescription.mockRejectedValueOnce(new Error('write failed'))
      await expect(writeGitDescription('/repo', 'changed')).rejects.toThrow(
        'write failed'
      )
    }
  )
})
