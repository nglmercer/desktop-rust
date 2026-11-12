// @vitest-environment node
import { describe, it } from 'vitest'
import { createRequire } from 'node:module'

import { TSESLint } from '@typescript-eslint/utils'
const RuleTester = TSESLint.RuleTester
import rule from '../react-no-unbound-dispatcher-props.js'

const parserOptions: TSESLint.ParserOptions = {
  ecmaVersion: 2018,
  sourceType: 'module',
  ecmaFeatures: {
    jsx: true,
  },
}

// ------------------------------------------------------------------------------
// Tests
// ------------------------------------------------------------------------------

RuleTester.describe = describe
RuleTester.it = it

describe('react-no-unbound-dispatcher-props', () => {
  const ruleTester = new RuleTester({
    parser: createRequire(import.meta.url).resolve('@typescript-eslint/parser'),
    parserOptions,
  })
  ruleTester.run('react-no-unbound-dispatcher-props', rule, {
    valid: [
      '<Resizable onReset={() => { this.props.dispatcher.resetSidebarWidth }} />',
    ],
    invalid: [
      {
        code: '<Resizable onReset={this.props.dispatcher.resetSidebarWidth} />',
        errors: [
          {
            messageId: 'unboundMethod',
            data: {
              text: 'this.props.dispatcher.resetSidebarWidth',
            },
          },
        ],
      },
    ],
  })
})
