import { describe, expect, it } from 'vitest'
import { isTestMenuEvent } from '../../src/lib/menu-event'

describe('test menu events', () => {
  it('recognizes test actions without accepting ordinary or invalid events', () => {
    expect(isTestMenuEvent('test-copilot-snapshot-card')).toBe(true)
    expect(isTestMenuEvent('boomtown')).toBe(true)
    expect(isTestMenuEvent('push')).toBe(false)
    expect(isTestMenuEvent('test-unknown')).toBe(false)
    expect(isTestMenuEvent(null)).toBe(false)
  })
})
