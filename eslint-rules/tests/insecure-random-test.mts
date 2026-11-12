// @vitest-environment node
import { describe, it } from 'vitest'

import { RuleTester } from 'eslint'
import rule from '../insecure-random.js'

const parserOptions: import('eslint').Linter.ParserOptions = {
  ecmaVersion: 2015,
  sourceType: 'module',
}

// ESLint exposes these runtime hooks, but its v8 type declarations omit them.
Reflect.set(RuleTester, 'describe', describe)
Reflect.set(RuleTester, 'it', it)

describe('insecure-random', () => {
  const ruleTester = new RuleTester({ parserOptions })
  ruleTester.run('insecure-random', rule, {
    valid: [
      'const b = crypto.randomBytes();',
      'const b = window.crypto.getRandomValues();',
    ],
    invalid: [
      {
        code: 'const b = Math.random();',
        errors: [{ messageId: 'mathRandomInsecure' }],
      },
      {
        code: `
      const crypto = require('crypto');

      const b = crypto.pseudoRandomBytes();`,
        errors: [{ messageId: 'pseudoRandomBytesInsecure' }],
      },
      {
        code: `
      const { pseudoRandomBytes } = require('crypto');

      const b = pseudoRandomBytes();

      `,
        errors: [{ messageId: 'pseudoRandomBytesInsecure' }],
      },
    ],
  })
})
