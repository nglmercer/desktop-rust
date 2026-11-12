import { readFileSync } from 'node:fs'
import path from 'node:path'
import vm from 'node:vm'
import { describe, expect, it } from 'vitest'
import fixture from './engine-expected.json'

interface IMatcherSandbox extends vm.Context {
  __desktopMatchRules: (parameters: string, branch: string) => string
}

function sandbox(): IMatcherSandbox {
  const context = vm.createContext({}) as IMatcherSandbox
  const code = readFileSync(
    path.resolve('rust/crates/core/src/repo_rules/re2js.js'),
    'utf8'
  )
  vm.runInContext(code, context, { timeout: 3000 })
  return context
}

describe('embedded branch-rule matcher', () => {
  it('matches the complete frozen dialect corpus without host APIs', () => {
    const context = sandbox()
    expect(context.TextEncoder).toBeUndefined()
    expect(context.TextDecoder).toBeUndefined()
    expect(context.process).toBeUndefined()
    expect(context.require).toBeUndefined()
    for (const item of fixture.cases) {
      let actual: unknown
      try {
        const result: unknown = JSON.parse(
          context.__desktopMatchRules(
            JSON.stringify([item.parameters]),
            item.branch
          )
        )
        expect(Array.isArray(result) && result.length === 1, item.id).toBe(true)
        actual = { matched: (result as boolean[])[0] }
      } catch (error) {
        const exception = error as { name: string; message: string }
        actual = {
          status: 'error',
          name: exception.name,
          message: exception.message,
        }
      }
      expect(actual, item.id).toEqual(item.outcome)
    }
  })

  it('rejects malformed inputs and does not match unknown operators', () => {
    const context = sandbox()
    expect(context.__desktopMatchRules('[]', 'branch')).toBe('[]')
    expect(
      context.__desktopMatchRules(
        JSON.stringify([
          { operator: 'starts_with', pattern: 'feature/', negate: false },
          { operator: 'unknown', pattern: 'ignored', negate: true },
        ]),
        'feature/topic'
      )
    ).toBe('[true,false]')
    for (const invalid of [
      'null',
      '{}',
      '[null]',
      '[{"operator":"regex","pattern":"a","negate":"false"}]',
    ]) {
      expect(() => context.__desktopMatchRules(invalid, 'branch')).toThrow()
    }
  })
})
