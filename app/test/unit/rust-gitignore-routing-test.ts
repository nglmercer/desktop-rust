import { beforeEach, describe, expect, it, vi } from 'vitest'

const native = vi.hoisted(() => ({
  transport: 'wry',
  action: vi.fn(),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: native.transport,
    repositoryGitIgnore: native.action,
  }),
}))
vi.mock('../../src/lib/git/config', () => ({ getConfigValue: vi.fn() }))

import {
  appendIgnoreFile,
  appendIgnoreRule,
  readGitIgnoreAtRoot,
  saveGitIgnore,
} from '../../src/lib/git/gitignore'
import type { Repository } from '../../src/models/repository'

const repo = { path: '/repository' } as Repository

describe.each(['wry', 'ws'])('Rust ignore file routing via %s', transport => {
  beforeEach(() => {
    vi.clearAllMocks()
    native.transport = transport
  })

  it('reads, saves, appends patterns, and escapes filenames in the backend', async () => {
    native.action.mockResolvedValueOnce('one\n').mockResolvedValue(null)
    await expect(readGitIgnoreAtRoot(repo)).resolves.toBe('one\n')
    await saveGitIgnore(repo, 'new text')
    await appendIgnoreRule(repo, ['one', 'two'])
    await appendIgnoreFile(repo, ['[file].txt', 'ignore!.log'])
    expect(native.action.mock.calls).toEqual([
      ['/repository', { type: 'read' }],
      ['/repository', { type: 'save', text: 'new text' }],
      ['/repository', { type: 'append', text: 'one\ntwo' }],
      [
        '/repository',
        { type: 'append', text: '\\[file\\].txt\nignore\\!.log' },
      ],
    ])
  })

  it('propagates backend failures without retrying legacy filesystem access', async () => {
    const error = new Error('ignore file changed')
    native.action.mockRejectedValueOnce(error)
    await expect(appendIgnoreRule(repo, 'entry')).rejects.toBe(error)
    expect(native.action).toHaveBeenCalledOnce()
  })
})
