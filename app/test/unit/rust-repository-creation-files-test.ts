import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  transport: 'ws',
  writeCreationFile: vi.fn(),
  writeFile: vi.fn(),
  readFile: vi.fn(),
  readdir: vi.fn(),
  listTemplates: vi.fn(),
  readTemplate: vi.fn(),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    writeRepositoryCreationFile: mocks.writeCreationFile,
  }),
}))
vi.mock('../../src/ui/add-repository/bundled-templates', () => ({
  listBundledGitIgnores: mocks.listTemplates,
  readBundledTemplate: mocks.readTemplate,
}))
vi.mock('fs/promises', () => ({
  writeFile: mocks.writeFile,
  readFile: mocks.readFile,
  readdir: mocks.readdir,
  default: {
    writeFile: mocks.writeFile,
    readFile: mocks.readFile,
    readdir: mocks.readdir,
  },
}))

import { writeDefaultReadme } from '../../src/ui/add-repository/write-default-readme'
import { writeGitAttributes } from '../../src/ui/add-repository/git-attributes'
import { writeGitIgnore } from '../../src/ui/add-repository/gitignores'
import { writeLicense } from '../../src/ui/add-repository/licenses'

describe('new repository files', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = 'ws'
    mocks.readdir.mockResolvedValue(['Node.gitignore'])
    mocks.readFile.mockResolvedValue('node_modules/\n')
    mocks.listTemplates.mockResolvedValue(['Node.gitignore'])
    mocks.readTemplate.mockResolvedValue('node_modules/\n')
  })

  it.each(['ws', 'wry'])(
    'writes rendered files through %s',
    async transport => {
      mocks.transport = transport
      await writeDefaultReadme('/repo', 'Project', 'Description')
      await writeGitIgnore('/repo', 'Node')
      await writeLicense(
        '/repo',
        {
          name: 'Example',
          featured: false,
          hidden: false,
          body: 'Copyright [year] {fullname}',
        },
        {
          fullname: 'Author',
          email: '',
          project: '',
          description: '',
          year: '2026',
        }
      )
      await writeGitAttributes('/repo')
      expect(mocks.writeCreationFile.mock.calls).toEqual([
        ['/repo', 'readme', '# Project\nDescription\n'],
        ['/repo', 'gitignore', 'node_modules/\n'],
        ['/repo', 'license', 'Copyright 2026 Author'],
        [
          '/repo',
          'git_attributes',
          '# Auto detect text files and perform LF normalization\n* text=auto\n',
        ],
      ])
      expect(mocks.writeFile).not.toHaveBeenCalled()
      expect(mocks.readdir).not.toHaveBeenCalled()
      expect(mocks.readFile).not.toHaveBeenCalled()
      expect(mocks.readTemplate).toHaveBeenCalledWith(
        'static/gitignore/Node.gitignore'
      )
    }
  )

  it('propagates Rust failures and retains the Electron fallback', async () => {
    mocks.writeCreationFile.mockRejectedValueOnce(new Error('write failed'))
    await expect(writeDefaultReadme('/repo', 'Project')).rejects.toThrow(
      'write failed'
    )
    expect(mocks.writeFile).not.toHaveBeenCalled()
    mocks.transport = 'none'
    await writeDefaultReadme('/repo', 'Project')
    expect(mocks.writeFile).toHaveBeenCalledWith(
      expect.stringMatching(/README\.md$/),
      '# Project\n'
    )
    await writeGitIgnore('/repo', 'Node')
    expect(mocks.readFile).toHaveBeenCalledWith(
      expect.stringMatching(/Node\.gitignore$/),
      'utf8'
    )
    expect(mocks.writeFile).toHaveBeenCalledWith(
      expect.stringMatching(/\.gitignore$/),
      'node_modules/\n'
    )
  })
})
