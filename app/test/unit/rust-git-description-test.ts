import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  transport: 'ws',
  description: vi.fn(),
  setDescription: vi.fn(),
  readFile: vi.fn(),
  writeFile: vi.fn(),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    repositoryGitDescription: mocks.description,
    setRepositoryGitDescription: mocks.setDescription,
  }),
}))
vi.mock('fs/promises', () => ({
  readFile: mocks.readFile,
  writeFile: mocks.writeFile,
  default: { readFile: mocks.readFile, writeFile: mocks.writeFile },
}))
import {
  DefaultGitDescription,
  getGitDescription,
  writeGitDescription,
} from '../../src/lib/git/description'

describe('repository Git description', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = 'ws'
  })

  it.each(['ws', 'wry'])(
    'uses the scoped Rust reader on %s',
    async transport => {
      mocks.transport = transport
      mocks.description.mockResolvedValueOnce('  project\n')
      await expect(getGitDescription('/repo')).resolves.toBe('  project\n')
      expect(mocks.description).toHaveBeenCalledWith('/repo')
      expect(mocks.readFile).not.toHaveBeenCalled()
      mocks.description.mockRejectedValueOnce(new Error('disconnected'))
      await expect(getGitDescription('/repo')).rejects.toThrow('disconnected')
    }
  )

  it('retains the Electron fallback for missing and default descriptions', async () => {
    mocks.transport = 'none'
    mocks.readFile
      .mockResolvedValueOnce(DefaultGitDescription)
      .mockRejectedValueOnce(new Error('missing'))
    await expect(getGitDescription('/repo')).resolves.toBe('')
    await expect(getGitDescription('/repo')).resolves.toBe('')
    expect(mocks.description).not.toHaveBeenCalled()
  })
  it.each(['ws', 'wry'])(
    'writes via the Rust repository operation on %s',
    async transport => {
      mocks.transport = transport
      mocks.setDescription.mockResolvedValueOnce(undefined)
      await writeGitDescription('/repo', 'project 🦀\n')
      expect(mocks.setDescription).toHaveBeenCalledWith('/repo', 'project 🦀\n')
      expect(mocks.writeFile).not.toHaveBeenCalled()
      mocks.setDescription.mockRejectedValueOnce(new Error('write failed'))
      await expect(writeGitDescription('/repo', 'changed')).rejects.toThrow(
        'write failed'
      )
    }
  )
})
