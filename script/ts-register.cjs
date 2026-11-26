// A minimal CommonJS loader for TypeScript sources, used by development and release scripts
// (`node --require ./script/ts-register.cjs`). It compiles each file
// with the TypeScript compiler API compatibility package, without
// type-checking. TypeScript 7 is the type checker, but does not expose the
// compiler API needed by this CommonJS loader. Scripts are still CommonJS-oriented; new test files run
// through Vitest and never use this hook.
'use strict'

const fs = require('node:fs')
const Module = require('node:module')
const path = require('node:path')
const ts = require('@typescript/compiler-api')

const configPath = path.join(__dirname, '..', 'tsconfig.runner.json')
const config = ts.readConfigFile(configPath, ts.sys.readFile)
if (config.error) {
  throw new Error(
    ts.flattenDiagnosticMessageText(config.error.messageText, '\n')
  )
}
const { options } = ts.parseJsonConfigFileContent(
  config.config,
  ts.sys,
  path.dirname(configPath)
)
const compilerOptions = {
  ...options,
  module: ts.ModuleKind.CommonJS,
  sourceMap: false,
  inlineSourceMap: true,
  inlineSources: true,
  declaration: false,
  noEmit: false,
  outDir: undefined,
}

function compile(module, filename) {
  const source = fs.readFileSync(filename, 'utf8')
  const { outputText } = ts.transpileModule(source, {
    compilerOptions,
    fileName: filename,
    reportDiagnostics: false,
  })
  module._compile(outputText, filename)
}

Module._extensions['.ts'] = compile
Module._extensions['.tsx'] = compile
