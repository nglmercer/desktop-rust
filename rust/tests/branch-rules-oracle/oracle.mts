import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { readFile, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { createRequire } from 'node:module'
import { build } from 'vite'
import ts from 'typescript'
import { matcherSource } from './matcher-source.mts'

const here = path.dirname(fileURLToPath(import.meta.url)),
  root = path.resolve(here, '../../..'),
  require = createRequire(import.meta.url)
const currentRe2js = process.argv.includes('--current-re2js')
const rendererPackage = path.join(root, 'app/node_modules/re2js')
const rendererMetadata = currentRe2js
  ? JSON.parse(
      await readFile(path.join(rendererPackage, 'package.json'), 'utf8')
    )
  : null
if (currentRe2js) assert.equal(typeof rendererMetadata.module, 'string')
const re2js = currentRe2js
  ? path.join(rendererPackage, rendererMetadata.module)
  : path.join(path.dirname(require.resolve('re2js')), 'index.esm.js')
const engineVersion = currentRe2js
  ? `re2js@${rendererMetadata.version}`
  : 're2js@2.0.1'
const files: Record<string, string> = {
  checker: 'app/src/ui/lib/branch-name-rule-validation.tsx',
  helper: 'app/src/lib/helpers/repo-rules.ts',
  model: 'app/src/models/repo-rules.ts',
  api: 'app/src/lib/api.ts',
  repository: 'app/src/models/repository.ts',
  account: 'app/src/models/account.ts',
  getAccount: 'app/src/lib/get-account-for-repository.ts',
  endpoint: 'app/src/lib/endpoint-capabilities.ts',
  fatal: 'app/src/lib/fatal-error.ts',
}
const sources: Record<string, string> = Object.fromEntries(
  await Promise.all(
    Object.entries(files).map(async ([key, filename]) => [
      key,
      await readFile(path.join(root, filename), 'utf8'),
    ])
  )
)
const trees: Record<string, ts.SourceFile> = Object.fromEntries(
  Object.entries(sources).map(([key, source]) => [
    key,
    ts.createSourceFile(files[key], source, ts.ScriptTarget.Latest, true),
  ])
)
function declaration(key: string, name: string): string {
  const found = trees[key].statements.find(
    node =>
      (ts.isEnumDeclaration(node) || ts.isFunctionDeclaration(node)) &&
      node.name?.text === name
  )
  assert(found, `Missing authoritative declaration ${name} in ${files[key]}`)
  return found.getText(trees[key])
}
function method(name: string): string {
  const api = trees.api.statements
    .filter(ts.isClassDeclaration)
    .find(node => node.name?.text === 'API')
  const found = api?.members.find(
    node => node.name?.getText(trees.api) === name
  )
  assert(found, `Missing authoritative API method ${name}`)
  return found.getText(trees.api)
}
const boundary = 'globalThis.__desktopBranchRulesOracle'
const modules: Record<string, string> = {
  api: `${declaration('api', 'APIRepoRuleType')}\n${declaration(
    'api',
    'APIRepoRuleMetadataOperator'
  )}\nconst envEndpoint = undefined;\n${declaration(
    'api',
    'getDotComAPIEndpoint'
  )}\nconst log={info:(...args)=>${boundary}.logs.push(args[0])};\nconst parsedResponse=async response=>response;\nconst isRulesetsNotEnabledError=error=>error.kind==='rules_disabled';\nconst isNotFoundApiError=error=>error.kind==='not_found';\nexport class API { static fromAccount(account){return new API()} async ghRequest(verb,requestPath){ const state=${boundary};state.apiCalls.push({method:verb,path:requestPath});if(state.input.api_error)throw Object.assign(new Error('fixture API failure'),{kind:state.input.api_error});return state.input.rules??[]; } ${method(
    'fetchRepoRulesForBranch'
  )} }`,
  repository: `export class Repository {}\nexport class GitHubRepository {constructor(fields){Object.assign(this,fields)}}\n${declaration(
    'repository',
    'isRepositoryWithGitHubRepository'
  )}\n${declaration('repository', 'getRepositoryAccountIdentity')}`,
  account: `${declaration('account', 'getAccountKey')}\n${declaration(
    'account',
    'isSameAccount'
  )}`,
  getAccount: `import {getRepositoryAccountIdentity} from 'oracle:repository';import {isSameAccount} from 'oracle:account';\n${declaration(
    'getAccount',
    'getAccountForRepository'
  )}`,
  git: `export async function getBooleanConfigValue(repository,key){const state=${boundary};state.gitReads.push(key);return state.input.signing_enabled??false}`,
  ui: 'export function createElement(){throw new Error("Oracle attempted UI rendering")} export const Row=createElement,InputError=createElement,InputWarning=createElement;',
}
const aliases = new Map(
  [
    ['app/src/lib/api', 'api'],
    ['app/src/models/repository', 'repository'],
    ['app/src/models/account', 'account'],
    ['app/src/lib/get-account-for-repository', 'getAccount'],
    ['app/src/lib/git', 'git'],
    ['app/src/ui/lib/input-description/input-error', 'ui'],
    ['app/src/ui/lib/input-description/input-warning', 'ui'],
    ['app/src/ui/lib/row', 'ui'],
  ].map(([filename, value]) => [path.join(root, filename), value])
)
const matcher = await matcherSource(root)
const entry = `export {checkBranchNameRules} from ${JSON.stringify(path.join(root, files.checker))};export {parseRepoRules,useRepoRulesLogic} from ${JSON.stringify(path.join(root, files.helper))};export {GitHubRepository} from 'oracle:repository';export {toMatcher} from 'oracle:matcher';`
const compiled = await build({
  configFile: false,
  root,
  publicDir: false,
  logLevel: 'silent',
  plugins: [
    {
      name: 'branch-rules-controlled-boundaries',
      enforce: 'pre',
      resolveId(id, importer) {
        if (id === 'oracle:entry') return '\0oracle:entry'
        if (id.startsWith('oracle:')) return `\0${id}`
        if (id === 'react') return '\0oracle:ui'
        if (id === 're2js') return re2js
        if (id === 'semver') return require.resolve('semver')
        const key =
          id.startsWith('.') && importer
            ? path
                .resolve(path.dirname(importer), id)
                .replace(/\.(ts|tsx)$/, '')
            : id
        const replacement = aliases.get(key)
        if (replacement) return `\0oracle:${replacement}`
      },
      load(id) {
        if (id === '\0oracle:entry') return entry
        if (id === '\0oracle:matcher') return matcher
        if (id.startsWith('\0oracle:')) {
          const source = modules[id.slice('\0oracle:'.length)]
          assert(source, `Unknown oracle module ${id}`)
          return ts.transpileModule(source, {
            compilerOptions: {
              target: ts.ScriptTarget.ES2020,
              module: ts.ModuleKind.ESNext,
            },
          }).outputText
        }
      },
    },
  ],
  build: {
    ssr: true,
    write: false,
    minify: false,
    target: 'es2020',
    rollupOptions: {
      input: 'oracle:entry',
      preserveEntrySignatures: 'strict',
      output: { format: 'es' },
    },
  },
  ssr: { noExternal: true },
})
assert(!Array.isArray(compiled) && 'output' in compiled)
assert.equal(compiled.output.length, 1)
assert.equal(compiled.output[0].type, 'chunk')
const oracle = await import(
  `data:text/javascript;base64,${Buffer.from(compiled.output[0].code).toString(
    'base64'
  )}`
)
Object.assign(globalThis, {
  localStorage: { getItem: () => null, setItem: () => {} },
})
interface IOracleInput {
  id: string
  branch: string
  account?: Record<string, unknown> | null
  github?: Record<string, unknown> | null
  assigned_login?: string | null
  rulesets?: Array<{ id: number; [key: string]: unknown }>
  rules?: Array<{
    type: string
    parameters?: { operator?: unknown; pattern?: unknown; negate?: unknown }
  }>
  expected_check?: string
  expected_pattern?: string
}
function errorValue(error: unknown) {
  const exception = error as { name: string; message: string }
  return {
    status: 'error' as const,
    name: exception.name,
    message: exception.message,
  }
}
async function evaluate(input: IOracleInput) {
  const state = {
    input,
    apiCalls: [] as unknown[],
    gitReads: [] as unknown[],
    logs: [] as unknown[],
  }
  Reflect.set(globalThis, '__desktopBranchRulesOracle', state)
  const account =
    input.account === null
      ? null
      : {
          login: 'tester',
          endpoint: 'https://api.github.com',
          plan: 'pro',
          ...input.account,
        }
  const metadata =
    input.github === null
      ? null
      : new oracle.GitHubRepository({
          endpoint: 'https://api.github.com',
          owner: { login: 'owner' },
          name: 'repo',
          isPrivate: false,
          ...input.github,
        })
  const repository = {
    gitHubRepository: metadata,
    login:
      input.assigned_login === undefined
        ? (account?.login ?? null)
        : input.assigned_login,
  }
  const sets = new Map(
    (input.rulesets ?? []).map(ruleset => [ruleset.id, ruleset])
  )
  let check:
    | ReturnType<typeof errorValue>
    | { status: string; message?: string }
  try {
    const result = await oracle.checkBranchNameRules(
      input.branch,
      account ? [account] : [],
      repository,
      sets
    )
    check =
      result === null
        ? { status: 'pass' }
        : {
            status: result.isWarning ? 'warning' : 'blocked',
            message: result.error.message,
          }
  } catch (error) {
    check = errorValue(error)
  }
  const apiCalls = [...state.apiCalls],
    checkGitReads = [...state.gitReads]
  let parsed:
    | ReturnType<typeof errorValue>
    | {
        creation_restricted: boolean
        pattern_status: string
        failed: unknown
        bypassed: unknown
      }
  try {
    const result = await oracle.parseRepoRules(
        input.rules ?? [],
        sets,
        repository
      ),
      failures = result.branchNamePatterns.getFailedRules(input.branch)
    parsed = {
      creation_restricted: result.creationRestricted,
      pattern_status: failures.status,
      failed: failures.failed,
      bypassed: failures.bypassed,
    }
  } catch (error) {
    parsed = errorValue(error)
  }
  return {
    id: input.id,
    check,
    parsed,
    api_calls: apiCalls,
    check_git_reads: checkGitReads,
  }
}
const corpus: { version: number; cases: IOracleInput[] } = JSON.parse(
  await readFile(path.join(here, 'cases.json'), 'utf8')
)
assert.equal(corpus.version, 1)
const results = []
for (const input of corpus.cases) {
  const result = await evaluate(input)
  if (input.expected_check)
    assert.equal(result.check.status, input.expected_check, input.id)
  if (input.expected_pattern)
    assert.equal(
      'pattern_status' in result.parsed
        ? result.parsed.pattern_status
        : undefined,
      input.expected_pattern,
      input.id
    )
  results.push(result)
}
const current = { version: 1, engine: engineVersion, cases: results }
const engineCases = []
const seen = new Set()
for (const input of corpus.cases)
  for (const rule of input.rules ?? []) {
    const parameters = rule.parameters
    if (
      rule.type !== 'branch_name_pattern' ||
      !parameters ||
      typeof parameters.operator !== 'string' ||
      typeof parameters.pattern !== 'string' ||
      typeof parameters.negate !== 'boolean'
    )
      continue
    const key = JSON.stringify([
      parameters.operator,
      parameters.pattern,
      parameters.negate,
      input.branch,
    ])
    if (seen.has(key)) continue
    seen.add(key)
    let outcome: ReturnType<typeof errorValue> | { matched: boolean }
    try {
      outcome = { matched: oracle.toMatcher(parameters)(input.branch) }
    } catch (error) {
      outcome = errorValue(error)
    }
    engineCases.push({
      id: input.id,
      parameters,
      branch: input.branch,
      outcome,
    })
  }
const engineCurrent = { version: 1, engine: engineVersion, cases: engineCases }
const mode = process.argv[2] ?? '--check'
if (mode === '--record') {
  assert(
    !currentRe2js,
    'The historical corpus must be recorded with pinned RE2JS 2.0.1'
  )
  await writeFile(
    path.join(here, 'expected.json'),
    `${JSON.stringify(current, null, 2)}\n`
  )
  await writeFile(
    path.join(here, 'engine-expected.json'),
    `${JSON.stringify(engineCurrent, null, 2)}\n`
  )
  process.stdout.write(
    `Recorded ${results.length} authoritative legacy branch-rule cases.\n`
  )
} else if (mode === '--check') {
  const expected = JSON.parse(
    await readFile(path.join(here, 'expected.json'), 'utf8')
  )
  if (currentRe2js) {
    assert.deepEqual(current.cases, expected.cases)
    const engineExpected = JSON.parse(
      await readFile(path.join(here, 'engine-expected.json'), 'utf8')
    )
    assert.deepEqual(engineCurrent.cases, engineExpected.cases)
  } else {
    assert.deepEqual(current, expected)
    assert.deepEqual(
      engineCurrent,
      JSON.parse(
        await readFile(path.join(here, 'engine-expected.json'), 'utf8')
      )
    )
  }
  process.stdout.write(
    `PASS: ${results.length} branch-rule cases (${current.engine}).\n`
  )
} else if (mode === '--json') {
  process.stdout.write(
    `${JSON.stringify({
      ...current,
      source_sha256: Object.fromEntries(
        Object.entries(sources).map(([key, value]) => [
          files[key],
          createHash('sha256').update(value).digest('hex'),
        ])
      ),
    })}\n`
  )
} else if (mode === '--evaluate') {
  const chunks = []
  for await (const chunk of process.stdin) chunks.push(chunk)
  const input = JSON.parse(Buffer.concat(chunks).toString('utf8'))
  let output:
    | Awaited<ReturnType<typeof evaluate>>
    | Array<Awaited<ReturnType<typeof evaluate>>>
  if (Array.isArray(input)) {
    output = []
    for (const item of input) output.push(await evaluate(item))
  } else output = await evaluate(input)
  process.stdout.write(`${JSON.stringify(output)}\n`)
} else
  throw new Error(
    'Usage: node --import tsx oracle.mts --check|--record|--json|--evaluate'
  )
