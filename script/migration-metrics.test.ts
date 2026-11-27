// @vitest-environment node
import { describe, expect, it } from 'vitest'
import { compareWithBaseline } from './migration-metrics'

describe('migration baseline comparison', () => {
  it('passes when nothing moved', () => {
    expect(compareWithBaseline({ a: 5, b: 0 }, { a: 5, b: 0 })).toEqual({
      regressions: [],
      improvements: [],
    })
  })

  it('reports a number that went up as a regression', () => {
    const { regressions } = compareWithBaseline({ a: 6 }, { a: 5 })
    expect(regressions).toEqual(['a: 6 is above the baseline 5'])
  })

  it('reports a number that went down so the baseline can be lowered', () => {
    const { regressions, improvements } = compareWithBaseline(
      { a: 3 },
      { a: 5 }
    )
    expect(regressions).toEqual([])
    expect(improvements).toEqual(['a: 3 is below the baseline 5'])
  })

  it('treats a new measure with no baseline as a regression', () => {
    expect(compareWithBaseline({ fresh: 1 }, {}).regressions).toEqual([
      'fresh: 1 has no baseline entry',
    ])
  })

  it('treats a baseline measure removed from current metrics as stale', () => {
    expect(compareWithBaseline({}, { retired: 2 }).regressions).toEqual([
      'retired: baseline entry 2 is stale',
    ])
  })
})
