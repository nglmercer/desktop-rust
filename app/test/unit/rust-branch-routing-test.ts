import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  transport: 'wry',
  branches: vi.fn(),
  git: vi.fn(),
  scope: vi.fn(
    async (_path: string, _kind: string, run: () => Promise<unknown>) => run()
  ),
}))

vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    repositoryBranches: mocks.branches,
  }),
}))
vi.mock('../../src/lib/git/repository-operation', () => ({
  withRepositoryGitOperation: mocks.scope,
}))
vi.mock('../../src/lib/git/core', () => ({ git: mocks.git }))

import {
  getBranches,
  getBranchesDifferingFromUpstream,
} from '../../src/lib/git/for-each-ref'
import { BranchType } from '../../src/models/branch'
import type { Repository } from '../../src/models/repository'

const repository = { path: '/repo-a' } as Repository

describe('Rust branch enumeration', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = 'wry'
  })

  it('uses the bridge without the native lease and propagates failures', async () => {
    mocks.transport = 'ws'
    mocks.branches.mockResolvedValueOnce([
      { ref: 'refs/heads/topic', name: 'topic', upstream: null, oid: 'abc' },
    ])
    await expect(getBranches(repository)).resolves.toMatchObject([
      { name: 'topic', type: BranchType.Local },
    ])
    const failure = new Error('repository changed')
    mocks.branches.mockRejectedValueOnce(failure)
    await expect(getBranchesDifferingFromUpstream(repository)).rejects.toBe(
      failure
    )
    expect(mocks.scope).not.toHaveBeenCalled()
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('uses Git disambiguated names and upstreams under the read lease', async () => {
    mocks.branches.mockResolvedValue([
      {
        ref: 'refs/heads/topic',
        name: 'heads/topic',
        oid: 'abc',
        upstream: 'origin/topic',
      },
      {
        ref: 'refs/remotes/origin/topic',
        name: 'origin/topic',
        oid: 'def',
        upstream: null,
      },
    ])
    const branches = await getBranches(repository)
    expect(mocks.branches).toHaveBeenCalledWith('/repo-a', [
      'refs/heads',
      'refs/remotes',
    ])
    expect(mocks.scope).toHaveBeenCalledWith(
      '/repo-a',
      'read',
      expect.any(Function),
      true
    )
    expect(mocks.git).not.toHaveBeenCalled()
    expect(branches).toMatchObject([
      {
        name: 'heads/topic',
        upstream: 'origin/topic',
        tip: { sha: 'abc' },
        type: BranchType.Local,
      },
      {
        name: 'origin/topic',
        upstream: null,
        tip: { sha: 'def' },
        type: BranchType.Remote,
      },
    ])
  })

  it('passes an explicit prefix and falls back on Rust read errors', async () => {
    mocks.branches.mockRejectedValue(new Error('unavailable'))
    mocks.git.mockResolvedValue({
      stdout: '\0refs/remotes/origin/topic\0origin/topic\0\0abc\0\0\n',
      gitError: null,
    })
    const branches = await getBranches(repository, 'refs/remotes/origin/')
    expect(mocks.branches).toHaveBeenCalledWith('/repo-a', [
      'refs/remotes/origin/',
    ])
    expect(branches).toMatchObject([
      { name: 'origin/topic', type: BranchType.Remote },
    ])
    expect(mocks.git).toHaveBeenCalledOnce()
  })

  it('propagates read lease failures without falling back', async () => {
    const failure = new Error('ownership fence')
    mocks.scope.mockRejectedValueOnce(failure)
    await expect(getBranches(repository)).rejects.toBe(failure)
    expect(mocks.branches).not.toHaveBeenCalled()
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('returns only non-current branches that differ from an existing remote upstream', async () => {
    mocks.branches.mockResolvedValue([
      {
        ref: 'refs/heads/ahead',
        oid: 'local',
        upstream_ref: 'refs/remotes/origin/ahead',
        current: false,
      },
      {
        ref: 'refs/heads/current',
        oid: 'local',
        upstream_ref: 'refs/remotes/origin/current',
        current: true,
      },
      {
        ref: 'refs/heads/equal',
        oid: 'same',
        upstream_ref: 'refs/remotes/origin/equal',
        current: false,
      },
      {
        ref: 'refs/heads/missing',
        oid: 'local',
        upstream_ref: 'refs/remotes/origin/missing',
        current: false,
      },
      {
        ref: 'refs/heads/no-upstream',
        oid: 'local',
        upstream_ref: null,
        current: false,
      },
      {
        ref: 'refs/heads/local-upstream',
        oid: 'local',
        upstream_ref: 'refs/heads/ahead',
        current: false,
      },
      {
        ref: 'refs/remotes/origin/ahead',
        oid: 'remote',
        upstream_ref: null,
        current: false,
      },
      {
        ref: 'refs/remotes/origin/current',
        oid: 'remote',
        upstream_ref: null,
        current: false,
      },
      {
        ref: 'refs/remotes/origin/equal',
        oid: 'same',
        upstream_ref: null,
        current: false,
      },
    ])
    await expect(getBranchesDifferingFromUpstream(repository)).resolves.toEqual(
      [
        {
          ref: 'refs/heads/ahead',
          sha: 'local',
          upstreamRef: 'refs/remotes/origin/ahead',
          upstreamSha: 'remote',
        },
      ]
    )
    expect(mocks.scope).toHaveBeenCalledWith(
      '/repo-a',
      'read',
      expect.any(Function),
      true
    )
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('preserves legacy tracking behavior when the Rust read fails', async () => {
    mocks.branches.mockRejectedValue(new Error('unavailable'))
    mocks.git.mockResolvedValue({
      stdout:
        '\0refs/heads/topic\0local\0refs/remotes/origin/topic\0\0 \0\n' +
        '\0refs/remotes/origin/HEAD\0remote\0\0refs/remotes/origin/topic\0 \0\n' +
        '\0refs/remotes/origin/topic\0remote\0\0\0 \0\n',
      gitError: null,
    })
    await expect(getBranchesDifferingFromUpstream(repository)).resolves.toEqual(
      [
        {
          ref: 'refs/heads/topic',
          sha: 'local',
          upstreamRef: 'refs/remotes/origin/topic',
          upstreamSha: 'remote',
        },
      ]
    )
    expect(mocks.git).toHaveBeenCalledOnce()
  })
})
