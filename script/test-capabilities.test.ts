// @vitest-environment node
import { describe, expect, it } from 'vitest'
import { resolve } from 'node:path'
import { existsSync, rmSync } from 'node:fs'
import {
  createCapableGitDirectory,
  filterByGitCapabilities,
  gitHasCapability,
  testGitExecutable,
} from './test-capabilities'

const missingGit = { LOCAL_GIT_DIRECTORY: '/nonexistent/git-directory' }
const resolvePath = (file: string) => resolve('/repo', file)

describe('test Git capabilities', () => {
  it('uses the Git that dugite would run', () => {
    expect(testGitExecutable({})).toBe('git')
    expect(testGitExecutable(missingGit)).toBe(
      '/nonexistent/git-directory/bin/git'
    )
  })

  it('provides a dugite-shaped layout for the system Git on POSIX', () => {
    if (process.platform === 'win32') {
      return
    }
    const layout = createCapableGitDirectory({})
    expect(layout).toBeDefined()
    expect(existsSync(`${layout?.directory}/bin/git`)).toBe(true)
    rmSync(layout!.directory, { recursive: true, force: true })
  })

  it('reports no capability for an unusable Git', () => {
    expect(gitHasCapability('git-lfs', missingGit)).toBe(false)
    expect(gitHasCapability('remote-url-reset', missingGit)).toBe(false)
  })

  it('skips only the suites that need a missing capability, with a reason', () => {
    const lfs = 'app/test/unit/git/lfs-test.ts'
    const other = 'app/test/unit/git/branch-test.ts'
    const { files, skipped } = filterByGitCapabilities(
      [lfs, other],
      resolvePath,
      missingGit
    )
    expect(files).toEqual([other])
    expect(skipped).toHaveLength(1)
    expect(skipped[0].file).toBe(lfs)
    expect(skipped[0].reason).toContain('git-lfs')
  })

  it('keeps every suite when nothing needs a capability', () => {
    const files = ['app/test/unit/git/branch-test.ts']
    expect(
      filterByGitCapabilities(files, resolvePath, missingGit).files
    ).toEqual(files)
  })
})
