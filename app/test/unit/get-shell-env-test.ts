import { beforeAll, describe, it } from 'vitest'
import assert from 'node:assert'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { getShellEnv } from '../../src/lib/hooks/get-shell-env'
import { SupportedHooksEnvShell } from '../../src/lib/hooks/config'
import { buildPrintenvz, getPrintenvzPath } from '../../../script/printenvz'

describe('getShellEnv', () => {
  beforeAll(() => buildPrintenvz(), 120_000)

  it.skipIf(__WIN32__)(
    'rejects valid-looking output if the shell exits unsuccessfully',
    async () => {
      const dir = await mkdtemp(join(tmpdir(), 'desktop-shell-env-test-'))
      const executable = join(dir, 'printenvz')
      const previousShell = process.env.SHELL
      try {
        await writeFile(
          executable,
          "#!/bin/sh\nprintf '%s\\n' '--printenvz--begin'\nprintf 'PATH=/example\\0\\n'\nprintf '%s\\n' '--printenvz--end'\nexec 1>&-\nsleep 0.1\nexit 7\n",
          { mode: 0o700 }
        )
        process.env.SHELL = '/bin/sh'
        await assert.rejects(
          getShellEnv(dir, undefined, executable),
          /child exited with code 7/
        )
      } finally {
        if (previousShell === undefined) {
          delete process.env.SHELL
        } else {
          process.env.SHELL = previousShell
        }
        await rm(dir, { recursive: true, force: true })
      }
    }
  )

  it.skipIf(__WIN32__)(
    'drains shell diagnostics while collecting the environment',
    async () => {
      const dir = await mkdtemp(join(tmpdir(), 'desktop-shell-env-test-'))
      const executable = join(dir, 'printenvz')
      const previousShell = process.env.SHELL
      try {
        await writeFile(
          executable,
          "#!/bin/sh\ni=0\nwhile [ $i -lt 4096 ]; do printf '%064d' \"$i\" >&2; i=$((i + 1)); done\nprintf '%s\\n' '--printenvz--begin'\nprintf 'PATH=/example\\0\\n'\nprintf '%s\\n' '--printenvz--end'\n",
          { mode: 0o700 }
        )
        process.env.SHELL = '/bin/sh'
        const result = await getShellEnv(dir, undefined, executable)
        assert.equal(result.kind, 'success')
        if (result.kind === 'success') {
          assert.equal(result.env.PATH, '/example')
        }
      } finally {
        if (previousShell === undefined) {
          delete process.env.SHELL
        } else {
          process.env.SHELL = previousShell
        }
        await rm(dir, { recursive: true, force: true })
      }
    },
    5_000
  )

  const shellKinds: ReadonlyArray<SupportedHooksEnvShell | undefined> =
    __WIN32__ ? ['git-bash', 'pwsh', 'powershell', 'cmd'] : [undefined]

  for (const shellKind of shellKinds) {
    const label = shellKind ?? 'default shell'
    it(`returns an env containing PATH (${label})`, async () => {
      const result = await getShellEnv(undefined, shellKind, getPrintenvzPath())

      assert.equal(result.kind, 'success')

      if (result.kind !== 'success') {
        return
      }

      const pathKey = Object.keys(result.env).find(
        k => k.toLowerCase() === 'path'
      )

      assert.notEqual(
        pathKey,
        undefined,
        `Expected env to contain a PATH key but got keys: ${Object.keys(
          result.env
        ).join(', ')}`
      )
    })
  }
})
