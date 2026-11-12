import { beforeEach, describe, expect, it, vi } from 'vitest'
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const mocks = vi.hoisted(() => ({
  transport: 'wry',
  aheadBehind: vi.fn(),
  revisions: vi.fn(),
  hasMerge: vi.fn(),
  git: vi.fn(),
  scope: vi.fn(
    async (_path: string, _kind: string, run: () => Promise<unknown>) => run()
  ),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    repositoryAheadBehind: mocks.aheadBehind,
    repositoryRevisionList: mocks.revisions,
    repositoryHasMergeCommits: mocks.hasMerge,
  }),
}))
vi.mock('../../src/lib/git/repository-operation', () => ({
  withRepositoryGitOperation: mocks.scope,
}))
vi.mock('../../src/lib/git/core', () => ({ git: mocks.git }))

import {
  doMergeCommitsExistAfterCommit,
  getAheadBehind,
  getCommitsInRange,
} from '../../src/lib/git/rev-list'
import type { Repository } from '../../src/models/repository'
const repository = { path: '/repo-a' } as Repository

describe('Rust ahead/behind lookup', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = 'wry'
  })

  it('uses the bridge and preserves a missing revision without a shell fallback', async () => {
    mocks.transport = 'ws'
    mocks.aheadBehind
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce({ ahead: 2, behind: 1 })
    await expect(
      getAheadBehind(repository, 'missing...main')
    ).resolves.toBeNull()
    await expect(getAheadBehind(repository, 'topic...main')).resolves.toEqual({
      ahead: 2,
      behind: 1,
    })
    const error = new Error('repository changed')
    mocks.aheadBehind.mockRejectedValueOnce(error)
    await expect(getAheadBehind(repository, 'topic...main')).rejects.toBe(error)
    expect(mocks.scope).not.toHaveBeenCalled()
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('uses the selected range under the repository read lease', async () => {
    mocks.aheadBehind.mockResolvedValue({ ahead: 3, behind: 2 })
    await expect(
      getAheadBehind(repository, 'local...origin/main')
    ).resolves.toEqual({ ahead: 3, behind: 2 })
    expect(mocks.aheadBehind).toHaveBeenCalledWith(
      '/repo-a',
      'local...origin/main'
    )
    expect(mocks.scope).toHaveBeenCalledWith(
      '/repo-a',
      'read',
      expect.any(Function),
      true
    )
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('preserves an empty result without repeating the query', async () => {
    mocks.aheadBehind.mockResolvedValue(null)
    await expect(getAheadBehind(repository, 'main...main')).resolves.toBeNull()
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('propagates Rust read errors without invoking the legacy Git runner', async () => {
    const error = new Error('unavailable')
    mocks.aheadBehind.mockRejectedValue(error)
    await expect(getAheadBehind(repository, 'topic...main')).rejects.toBe(error)
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('propagates lease errors without retrying', async () => {
    const error = new Error('ownership fence')
    mocks.scope.mockRejectedValueOnce(error)
    await expect(getAheadBehind(repository, 'topic...main')).rejects.toBe(error)
    expect(mocks.git).not.toHaveBeenCalled()
  })
})

describe('Rust revision and merge history', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = 'wry'
  })

  it('routes revision and merge queries through the bridge without shell fallback', async () => {
    mocks.transport = 'ws'
    mocks.revisions
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce(`${'a'.repeat(40)} first\n`)
    await expect(
      getCommitsInRange(repository, 'missing..main')
    ).resolves.toBeNull()
    await expect(getCommitsInRange(repository, 'base..main')).resolves.toEqual([
      { sha: 'a'.repeat(40), summary: 'first' },
    ])
    mocks.hasMerge.mockResolvedValueOnce(false)
    await expect(
      doMergeCommitsExistAfterCommit(repository, null)
    ).resolves.toBe(false)
    const error = new Error('repository changed')
    mocks.hasMerge.mockRejectedValueOnce(error)
    await expect(
      doMergeCommitsExistAfterCommit(repository, 'base')
    ).rejects.toBe(error)
    expect(mocks.scope).not.toHaveBeenCalled()
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('preserves rebase order for SHA-1 and SHA-256 object IDs', async () => {
    const first = 'a'.repeat(40)
    const second = 'b'.repeat(40)
    mocks.revisions.mockResolvedValue(
      `${first} first summary\n${second} second summary\n${'c'.repeat(
        64
      )} sha256 summary\n`
    )
    await expect(getCommitsInRange(repository, 'base..topic')).resolves.toEqual(
      [
        { sha: first, summary: 'first summary' },
        { sha: second, summary: 'second summary' },
        { sha: 'c'.repeat(64), summary: 'sha256 summary' },
      ]
    )
    expect(mocks.revisions).toHaveBeenCalledWith('/repo-a', 'base..topic')
    expect(mocks.scope).toHaveBeenCalledWith(
      '/repo-a',
      'read',
      expect.any(Function),
      true
    )
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('parses a real SHA-256 Git revision list', async () => {
    const directory = mkdtempSync(join(tmpdir(), 'desktop-sha256-history-'))
    try {
      execFileSync('git', ['init', '--object-format=sha256', '-q', directory])
      execFileSync('git', [
        '-C',
        directory,
        '-c',
        'user.name=Test',
        '-c',
        'user.email=test@example.invalid',
        'commit',
        '--allow-empty',
        '-m',
        'first commit',
      ])
      const sha = execFileSync('git', ['-C', directory, 'rev-parse', 'HEAD'], {
        encoding: 'utf8',
      }).trim()
      const output = execFileSync(
        'git',
        [
          '-C',
          directory,
          'rev-list',
          '--reverse',
          '--oneline',
          '--no-abbrev-commit',
          '--end-of-options',
          'HEAD',
          '--',
        ],
        { encoding: 'utf8' }
      )
      mocks.revisions.mockResolvedValue(output)
      await expect(getCommitsInRange(repository, 'HEAD')).resolves.toEqual([
        { sha, summary: 'first commit' },
      ])
      expect(sha).toHaveLength(64)
      expect(mocks.git).not.toHaveBeenCalled()
    } finally {
      rmSync(directory, { recursive: true, force: true })
    }
  })

  it('propagates revision read errors without invoking the legacy Git runner', async () => {
    const error = new Error('revision read failed')
    mocks.revisions.mockRejectedValue(error)
    await expect(getCommitsInRange(repository, 'missing..topic')).rejects.toBe(
      error
    )
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('propagates merge history errors without invoking the legacy Git runner', async () => {
    const error = new Error('merge history read failed')
    mocks.hasMerge.mockRejectedValue(error)
    await expect(doMergeCommitsExistAfterCommit(repository, null)).rejects.toBe(
      error
    )
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('uses the right revision for merge detection and preserves false', async () => {
    mocks.hasMerge.mockResolvedValueOnce(false).mockResolvedValueOnce(true)
    await expect(
      doMergeCommitsExistAfterCommit(repository, null)
    ).resolves.toBe(false)
    await expect(
      doMergeCommitsExistAfterCommit(repository, 'base')
    ).resolves.toBe(true)
    expect(mocks.hasMerge.mock.calls).toEqual([
      ['/repo-a', 'HEAD'],
      ['/repo-a', 'base..HEAD'],
    ])
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('does not bypass the repository lease', async () => {
    const error = new Error('ownership fence')
    mocks.scope.mockRejectedValueOnce(error)
    await expect(getCommitsInRange(repository, 'base..topic')).rejects.toBe(
      error
    )
    expect(mocks.git).not.toHaveBeenCalled()
  })
})
