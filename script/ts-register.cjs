// A minimal CommonJS loader for TypeScript sources, used by the Node test
// runner (`node --require ./script/ts-register.cjs`). It compiles each file
// with the TypeScript compiler itself, without type-checking, so that
// namespace and default imports keep TypeScript's live bindings: the legacy
// suites patch built-in modules (for example `Fs.watch`) after importing them.
'use strict'

const fs = require('node:fs')
const Module = require('node:module')
const path = require('node:path')
const ts = require('typescript')

const configPath = path.join(__dirname, '..', 'tsconfig.node-test.json')
const config = ts.readConfigFile(configPath, ts.sys.readFile)
if (config.error) {
  throw new Error(ts.flattenDiagnosticMessageText(config.error.messageText, '\n'))
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
