// @vitest-environment node

import { describe, it } from 'vitest'
import { createRequire } from 'node:module'
import { TSESLint } from '@typescript-eslint/utils'

const RuleTester = TSESLint.RuleTester
import rule from '../react-readonly-props-and-state.js'

// ------------------------------------------------------------------------------
// Tests
// ------------------------------------------------------------------------------

const ruleTester = new RuleTester({
  parser: createRequire(import.meta.url).resolve('@typescript-eslint/parser'),
})
RuleTester.describe = describe
RuleTester.it = it

describe('react-readonly-props-and-state', () => {
  ruleTester.run('react-readonly-props-and-state', rule, {
    valid: [
      {
        filename: 'app/src/ui/component.tsx',
        code: `
interface IBranchListItemProps {
  readonly name: string
}
`,
      },
      {
        filename: 'app/src/ui/component.tsx',
        code: `
interface IBranchListItemState {
  readonly name: string
}
`,
      },
      {
        filename: 'app/src/ui/component.tsx',
        code: `
interface ISomeOtherThing {
  name: string
}
`,
      },
      {
        filename: 'app/src/ui/component.tsx',
        code: `
interface ISomeState {
  readonly name: ReadonlyArray<string>
}
`,
      },
      {
        filename: 'app/src/ui/diff/helper.ts',
        code: `
  interface IState {
    diffLineIndex: number
    previousHunkOldEndLine: number | null
  }`,
      },
    ],
    invalid: [
      {
        filename: 'app/src/ui/component.tsx',
        code: `
interface IBranchListItemProps {
  name: string
}
      `,
        errors: [
          {
            messageId: 'signaturesShouldBeReadonly',
          },
        ],
      },
      {
        filename: 'app/src/ui/component.tsx',
        code: `
interface IBranchListItemProps {
  readonly items: Array<string>
}
      `,
        errors: [
          {
            messageId: 'arraySignaturesShouldBeReadonly',
          },
        ],
      },
      {
        filename: 'app/src/ui/component.tsx',
        code: `
interface IBranchListItemProps {
  readonly items: string[]
}
      `,
        errors: [
          {
            messageId: 'arraySignaturesShouldBeReadonly',
          },
        ],
      },
    ],
  })
})
