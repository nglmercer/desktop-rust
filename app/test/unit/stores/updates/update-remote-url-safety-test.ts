import { describe, expect, it, vi } from 'vitest'
import type { GitStore } from '../../../../src/lib/stores/git-store'
import type { IAPIRepository } from '../../../../src/lib/api'
import type { GitHubRepository } from '../../../../src/models/github-repository'
import { updateRemoteUrl } from '../../../../src/lib/stores/updates/update-remote-url'

describe('automatic remote URL updates', () => {
  it.each([
    'https://user:secret@github.com/my-user/my-repo',
    'https://@github.com/my-user/my-repo',
    'https://github.com/my-user/my-repo?access=secret',
    'https://github.com/my-user/my-repo#custom',
    'https://github.com/my-user/my-repo?',
    'https://github.com/my-user/my-repo#',
    'git@github.com:my-user/my-repo.git',
    'https://[malformed/my-user/my-repo',
    'https://github.com\\my-user/my-repo',
  ])('preserves locally configured URL data: %s', async url => {
    const setRemoteURL = vi.fn()
    const store = {
      defaultRemote: { name: 'origin', url },
      setRemoteURL,
    } as unknown as GitStore
    const repository = {
      cloneURL: 'https://github.com/my-user/my-repo',
    } as GitHubRepository
    const api = {
      clone_url: 'https://github.com/my-user/renamed',
    } as IAPIRepository

    await updateRemoteUrl(store, repository, api)
    expect(setRemoteURL).not.toHaveBeenCalled()
  })

  it('still updates an unchanged credential-free remote', async () => {
    const setRemoteURL = vi.fn()
    const store = {
      defaultRemote: { name: 'origin', url: 'https://github.com/my-user/my-repo' },
      setRemoteURL,
    } as unknown as GitStore
    const repository = {
      cloneURL: 'https://github.com/my-user/my-repo',
    } as GitHubRepository
    const api = {
      clone_url: 'https://github.com/my-user/renamed',
    } as IAPIRepository

    await updateRemoteUrl(store, repository, api)
    expect(setRemoteURL).toHaveBeenCalledWith('origin', api.clone_url)
  })
})
