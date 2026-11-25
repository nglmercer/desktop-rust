import { describe, it } from 'vitest'
import assert from 'node:assert'
import { GitError } from 'dugite'
import { git } from '../../../src/lib/git'
import { setupFixtureRepository } from '../../helpers/repositories'

describe('git/core', () => {
  describe('error handling', () => {
    it('does not throw for errors that were expected', async t => {
      const testRepoPath = await setupFixtureRepository(t, 'test-repo')

      const args = ['rev-list', '--left-right', '--count', 'some-ref', '--']
      const result = await git(args, testRepoPath, 'test', {
        expectedErrors: new Set([GitError.BadRevision]),
      })
      assert.equal(result.gitError, GitError.BadRevision)
    })

    it('throws for errors that were not expected', async t => {
      const testRepoPath = await setupFixtureRepository(t, 'test-repo')

      const args = ['rev-list', '--left-right', '--count', 'some-ref', '--']
      await assert.rejects(
        git(args, testRepoPath, 'test', {
          expectedErrors: new Set([GitError.SSHKeyAuditUnverified]),
        })
      )
    })
  })

  describe('exit code handling', () => {
    it('does not throw for exit codes that were expected', async t => {
      const repoPath = await setupFixtureRepository(t, 'test-repo')
      const args = ['rev-list', '--left-right', '--count', 'some-ref', '--']
      const result = await git(args, repoPath, 'test', {
        successExitCodes: new Set([128]),
      })
      assert.equal(result.exitCode, 128)
    })

    it('throws for exit codes that were not expected', async t => {
      const repoPath = await setupFixtureRepository(t, 'test-repo')
      const args = ['rev-list', '--left-right', '--count', 'some-ref', '--']
      await assert.rejects(
        git(args, repoPath, 'test', { successExitCodes: new Set([2]) })
      )
    })
  })
})
