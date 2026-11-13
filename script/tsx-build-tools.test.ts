// @vitest-environment node
import assert from 'node:assert/strict'
import { execFile } from 'node:child_process'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { promisify } from 'node:util'
import { it } from 'vitest'
import packageInfo from '../package.json'

const execFileAsync = promisify(execFile)

for (const [mode, script] of [
  ['development', 'compile:dev'],
  ['production', 'compile:prod'],
] as const) {
  it(`loads the ${mode} renderer configuration with tsx in webpack CLI`, async () => {
    const root = resolve('.')
    // Keep the fixture inside the project so webpack can resolve its loaders.
    const directory = await mkdtemp(join(root, '.webpack-tsx-check-'))
    try {
      const entry = join(directory, 'entry.js')
      const output = join(directory, 'output')
      const config = join(directory, 'config.ts')
      await writeFile(entry, 'console.log("tsx-webpack-fixture")\n')
      await writeFile(
        config,
        `const source = require(${JSON.stringify(
          join(root, 'app', `webpack.${mode}.ts`)
        )}).default[0]\n` +
          `export default [{ ...source, entry: { renderer: ${JSON.stringify(
            entry
          )} }, output: { ...source.output, path: ${JSON.stringify(
            output
          )} } }]\n`
      )
      assert.match(
        packageInfo.scripts[script],
        /node --import tsx node_modules\/webpack-cli\/bin\/cli\.js --disable-interpret/
      )
      await execFileAsync(
        process.execPath,
        [
          '--import',
          'tsx',
          join(root, 'node_modules/webpack-cli/bin/cli.js'),
          '--disable-interpret',
          '--config',
          config,
        ],
        { cwd: root, timeout: 60_000 }
      )
      assert.match(
        await readFile(join(output, 'renderer.js'), 'utf8'),
        /tsx-webpack-fixture/
      )
    } finally {
      await rm(directory, { recursive: true, force: true })
    }
  }, 65_000)
}
