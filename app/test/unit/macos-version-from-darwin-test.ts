import { describe, expect, it } from 'vitest'
import { macOSVersionFromDarwinRelease } from '../../src/lib/get-os'

describe('macOSVersionFromDarwinRelease', () => {
  it('maps Darwin kernels to macOS product versions', () => {
    expect(macOSVersionFromDarwinRelease('25.0.0')).toBe('26.0.0')
    expect(macOSVersionFromDarwinRelease('24.4.1')).toBe('15.4.1')
    expect(macOSVersionFromDarwinRelease('23.4.0')).toBe('14.4.0')
    expect(macOSVersionFromDarwinRelease('20.6.0')).toBe('11.6.0')
    expect(macOSVersionFromDarwinRelease('19.6.0')).toBe('10.15.6')
    expect(macOSVersionFromDarwinRelease('17.7.0')).toBe('10.13.7')
  })

  it('rejects unparseable releases', () => {
    expect(macOSVersionFromDarwinRelease('unknown')).toBeUndefined()
  })
})
