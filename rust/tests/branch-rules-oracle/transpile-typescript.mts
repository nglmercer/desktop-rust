import { execFile } from 'node:child_process'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'

function runTypeScriptCompiler(
  tsc: string,
  input: string,
  outDir: string
): Promise<void> {
  return new Promise((resolve, reject) => {
    execFile(
      process.execPath,
      [
        tsc,
        '--ignoreConfig',
        '--noCheck',
        '--target',
        'es2020',
        '--module',
        'esnext',
        '--outDir',
        outDir,
        input,
      ],
      (error, stdout, stderr) => {
        if (error === null) {
          resolve()
        } else {
          reject(new Error(`TypeScript transpile failed\n${stdout}${stderr}`))
        }
      }
    )
  })
}

/** Transpile an isolated in-memory TypeScript module with the TypeScript 7 compiler. */
export async function transpileTypeScript(
  root: string,
  fileName: string,
  source: string
): Promise<string> {
  const temporary = await mkdtemp(path.join(tmpdir(), 'desktop-oracle-ts-'))
  try {
    const baseName = path.basename(fileName).replace(/\.tsx?$/, '')
    const input = path.join(temporary, `${baseName}.ts`)
    const outDir = path.join(temporary, 'out')
    await writeFile(input, source)
    await runTypeScriptCompiler(
      path.join(root, 'node_modules/typescript/lib/tsc.js'),
      input,
      outDir
    )
    return await readFile(path.join(outDir, `${baseName}.js`), 'utf8')
  } finally {
    await rm(temporary, { recursive: true, force: true })
  }
}
