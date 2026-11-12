import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import path from 'node:path'
import ts from 'typescript'

/** Extract the production matcher and operator enum without importing the app. */
export async function matcherSource(root: string): Promise<string> {
  const files = [
    ['app/src/lib/api.ts', 'APIRepoRuleMetadataOperator'],
    ['app/src/lib/helpers/repo-rules.ts', 'toMatcher'],
  ] as const
  const declarations = await Promise.all(
    files.map(async ([file, name]) => {
      const source = await readFile(path.join(root, file), 'utf8')
      const tree = ts.createSourceFile(
        file,
        source,
        ts.ScriptTarget.Latest,
        true
      )
      const found = tree.statements.find(
        node =>
          (ts.isEnumDeclaration(node) || ts.isFunctionDeclaration(node)) &&
          node.name?.text === name
      )
      assert(found, `Missing production declaration ${name} in ${file}`)
      return found.getText(tree).replace(/^export\s+/, '')
    })
  )
  return ts.transpileModule(
    `import { RE2JS } from 're2js';\n${declarations[0]}\nexport ${declarations[1]}`,
    {
      compilerOptions: {
        target: ts.ScriptTarget.ES2020,
        module: ts.ModuleKind.ESNext,
      },
    }
  ).outputText
}
