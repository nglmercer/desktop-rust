import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  transport: 'ws',
  writeCreationFile: vi.fn(),
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

import { writeDefaultReadme } from '../../src/ui/add-repository/write-default-readme'
import { writeGitAttributes } from '../../src/ui/add-repository/git-attributes'
import { writeGitIgnore } from '../../src/ui/add-repository/gitignores'
import { writeLicense } from '../../src/ui/add-repository/licenses'

describe('new repository files', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = 'ws'
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
      expect(mocks.readTemplate).toHaveBeenCalledWith(
        'static/gitignore/Node.gitignore'
      )
    }
  )

  it('propagates Rust failures without a filesystem fallback', async () => {
    mocks.writeCreationFile.mockRejectedValueOnce(new Error('write failed'))
    await expect(writeDefaultReadme('/repo', 'Project')).rejects.toThrow(
      'write failed'
    )
  })
})
