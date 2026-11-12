// @vitest-environment node
import { afterEach, describe, expect, it } from 'vitest'
import {
  mkdir,
  mkdtemp,
  readFile,
  rm,
  unlink,
  writeFile,
} from 'node:fs/promises'
import { tmpdir } from 'node:os'
import * as Path from 'node:path'
import {
  DefaultGitDescription,
  getGitDescription,
  writeGitDescription,
} from '../../../src/lib/git/description'

let directory: string | undefined

async function repository(): Promise<string> {
  directory = await mkdtemp(Path.join(tmpdir(), 'desktop-description-test-'))
  await mkdir(Path.join(directory, '.git'))
  await writeFile(
    Path.join(directory, '.git', 'description'),
    DefaultGitDescription
  )
  return directory
}

afterEach(async () => {
  if (directory !== undefined)
    await rm(directory, { recursive: true, force: true })
  directory = undefined
})

describe('Git description Electron fallback', () => {
  it('returns empty for Git’s default description', async () => {
    await expect(getGitDescription(await repository())).resolves.toBe('')
  })

  it('returns empty when the description is missing', async () => {
    const path = await repository()
    await unlink(Path.join(path, '.git', 'description'))
    await expect(getGitDescription(path)).resolves.toBe('')
  })

  it('writes and reads custom text without changing it', async () => {
    const path = await repository()
    const text = '  repository 🦀\n\n'
    await writeGitDescription(path, text)
    await expect(getGitDescription(path)).resolves.toBe(text)
    await expect(
      readFile(Path.join(path, '.git', 'description'), 'utf8')
    ).resolves.toBe(text)
  })
})
