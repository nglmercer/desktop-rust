import assert from 'node:assert/strict'
import path from 'node:path'
import type { Project } from 'typescript/unstable/sync'
import {
  isEnumDeclaration,
  isFunctionDeclaration,
} from 'typescript/unstable/ast'
import { transpileTypeScript } from './transpile-typescript.mts'

/** Extract the production matcher and operator enum without importing the app. */
export async function matcherSource(
  root: string,
  project: Project
): Promise<string> {
  const files = [
    ['app/src/lib/api.ts', 'APIRepoRuleMetadataOperator'],
    ['app/src/lib/helpers/repo-rules.ts', 'toMatcher'],
  ] as const
  const declarations = files.map(([file, name]) => {
    const tree = project.program.getSourceFile(path.join(root, file))
    assert(tree, `Missing production source ${file}`)
    const found = tree.statements.find(
      node =>
        (isEnumDeclaration(node) || isFunctionDeclaration(node)) &&
        node.name?.text === name
    )
    assert(found, `Missing production declaration ${name} in ${file}`)
    return found.getText(tree).replace(/^export\s+/, '')
  })
  return transpileTypeScript(
    root,
    'branch-rules-matcher.ts',
    `import { RE2JS } from 're2js';\n${declarations[0]}\nexport ${declarations[1]}`
  )
}
