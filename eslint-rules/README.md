# Project-specific lint rules for GitHub Desktop

This document describes the project-specific checks and how to run them.

## About

This project uses rules from several sources. Checks specific to GitHub Desktop
live in `biome-rules/` and are configured in `biome.security.json`.

## Adding rules

If you wish to add a new rule specific to the project, ensure that there isn't an existing rule available from one of the existing plugins used in the project:

 - `@typescript-eslint/eslint-plugin`
 - `eslint-plugin-babel`
 - `eslint-plugin-jsdoc`
 - `eslint-plugin-json`
 - `eslint-plugin-react`
 
Project-specific rules are now implemented as Biome plugins in `biome-rules/`.
Run them with `npm run lint:security` and add regression tests in
`eslint-rules/tests/` while the broader lint migration is in progress.

How to write a plugin is out of scope for this documentation, but I've added some resources at the end of this page to help you get started.

## Checking locally

The Biome plugins can be checked through npm:

```
$ npm run lint:security
```

The plugins are TypeScript and are checked in strict mode by
`biome-rules/tsconfig.json`.

## Testing locally

Tests are added alongside each rule in the `eslint-rules/tests/` section, and can be run from the project root through npm:

```
$ npm run test:eslint
```

Each test suite is designed to exercise the relevant rule against code snippets that illustrate both valid and invalid code, and indicate which messages should be reported in case of failure.

Name test files `*-test.mts` and register migrated suites in
`vitest-suites.json`. Run the pinned Biome CLI against temporary fixtures.

The migrated `insecure-random`, `no-loosely-typed-webcontents-ipc`,
`react-no-unbound-dispatcher-props`, `react-readonly-props-and-state` and
`react-proper-lifecycle-methods`
checks live in `biome-rules/` and run
through `npm run lint:security`. Their tests
invoke the pinned Biome CLI against temporary fixtures. Suppressions must name
the specific plugin rule so they do not hide other diagnostics.

The lifecycle rule checks each component independently. Unlike the former
ESLint implementation, it does not reuse a previous component's types when
checking a later class. Unsupported parameter forms receive a diagnostic rather
than causing the linter to throw.

The recommended TypeScript lint preset remains enabled. When updating it,
compare the effective configuration (`npm run eslint -- --print-config <file>`)
before and after the upgrade, retaining checks removed from the preset and
using replacements for renamed rules. For example, `no-empty-object-type` with
`allowInterfaces: never` and `allowObjectTypes: always` preserves
`no-empty-interface` without adding the empty object type restriction from the
previously disabled `ban-types` rule. ESLint's core
`no-loss-of-precision` replaces the removed TypeScript extension rule.

The disabled `no-var-requires` rule maps to disabled `no-require-imports`:
Electron renderer callbacks and startup paths can require synchronous CommonJS
loading. This retains the previous policy rather than forcing asynchronous
imports or changing where those modules execute. Other newly recommended
checks, including `no-wrapper-object-types` and `no-unsafe-function-type`,
remain enabled.

If you wish to debug the rules using VSCode, add this action to the `configurations` array of the `.vscode/launch.json` settings:


```json
{
  "command": "npm run test:eslint",
  "name": "Test ESLint scripts",
  "request": "launch",
  "type": "node-terminal"
}
```

Running this command will attach the debugger, and allow you to step through the rule with the available test cases.

## Additional resources

 - [ESLint - Working with Rules](http://eslint.org/docs/developer-guide/working-with-rules)
 - [`typescript-estree` package](https://github.com/typescript-eslint/typescript-eslint/tree/master/packages/typescript-estree) - this is part of `@typescript-eslint` and allows for interop between Typescript code and the `estree` reference spec that ESLint uses for it's plugins.
