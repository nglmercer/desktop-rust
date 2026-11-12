// @vitest-environment node

import { describe, it } from 'vitest'
import { createRequire } from 'node:module'
import { TSESLint } from '@typescript-eslint/utils'
import plugin from '@typescript-eslint/eslint-plugin'

TSESLint.RuleTester.describe = describe
TSESLint.RuleTester.it = it

describe('preserves empty-interface checks without banning empty object types', () => {
  const ruleTester = new TSESLint.RuleTester({
    parser: createRequire(import.meta.url).resolve('@typescript-eslint/parser'),
  })

  const options = [{ allowInterfaces: 'never', allowObjectTypes: 'always' }]
  ruleTester.run(
    '@typescript-eslint/no-empty-object-type',
    plugin.rules['no-empty-object-type'],
    {
      valid: [
        'export type Empty = {}',
        'export interface IProps { readonly value: string }',
        `export interface IFirst { readonly first: string }
         export interface ISecond { readonly second: string }
         export interface ICombined extends IFirst, ISecond {}`,
      ].map(code => ({ code, options })),
      invalid: [
        {
          code: 'export interface IEmpty {}',
          options,
          errors: [{ messageId: 'noEmptyInterface' }],
        },
        {
          code: `export interface IBase { readonly value: string }
                 export interface IEmpty extends IBase {}`,
          options,
          errors: [{ messageId: 'noEmptyInterfaceWithSuper' }],
        },
      ],
    }
  )
})
