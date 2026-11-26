// @vitest-environment node
import assert from 'node:assert/strict'
import { execFile } from 'node:child_process'
import { existsSync } from 'node:fs'
import { cp, mkdtemp, readFile, rm, symlink } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, relative, resolve, sep } from 'node:path'
import { describe, it } from 'vitest'
import trampolinePackage from '../vendor/desktop-trampoline/package.json'

const tsc = resolve(__dirname, '../node_modules/typescript/lib/tsc.js')

function runTypeScriptCompiler(packageRoot: string): Promise<void> {
  return new Promise((resolvePromise, reject) => {
    execFile(
      process.execPath,
      [tsc, '-p', 'tsconfig.json'],
      { cwd: packageRoot },
      (error, stdout, stderr) => {
        if (error !== null) {
          reject(
            new Error(
              `TypeScript emit failed in ${packageRoot}\n${stdout}${stderr}`
            )
          )
        } else {
          resolvePromise()
        }
      }
    )
  })
}

describe('native module TypeScript output', () => {
  for (const { name, main, additionalFiles } of [
    { ...trampolinePackage, additionalFiles: [] },
  ]) {
    it(`emits the published entry points for ${name}`, async () => {
      const sourceRoot = resolve(__dirname, '../vendor', name)
      const temporaryRoot = await mkdtemp(join(tmpdir(), `desktop-${name}-`))
      const packageRoot = join(temporaryRoot, name)
      try {
        await cp(sourceRoot, packageRoot, {
          recursive: true,
          filter: source => {
            const rootEntry = relative(sourceRoot, source).split(sep)[0]
            return !['build', 'dist', 'node_modules'].includes(rootEntry)
          },
        })
        // The copied package cannot see the repository's @types/node through
        // normal parent-directory lookup in the temporary directory.
        await symlink(
          resolve(__dirname, '../node_modules'),
          join(temporaryRoot, 'node_modules'),
          'junction'
        )

        await runTypeScriptCompiler(packageRoot)

        const config = JSON.parse(
          await readFile(join(packageRoot, 'tsconfig.json'), 'utf8')
        ) as { compilerOptions?: { declaration?: boolean } }
        const expectedFiles = [
          main,
          ...(config.compilerOptions?.declaration === true
            ? [main.replace(/\.js$/, '.d.ts')]
            : []),
          ...additionalFiles,
        ]
        for (const file of expectedFiles) {
          assert.ok(
            existsSync(join(packageRoot, file)),
            `Expected ${name} to emit ${file}`
          )
        }
      } finally {
        await rm(temporaryRoot, { recursive: true, force: true })
      }
    }, 60_000)
  }
})
