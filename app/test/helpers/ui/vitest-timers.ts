import { vi } from 'vitest'
import { act } from 'react-dom/test-utils'

type TimerApi = 'Date' | 'setTimeout'

export function enableTestTimers(apis: ReadonlyArray<TimerApi>, now?: number) {
  vi.useFakeTimers({ toFake: [...apis] })
  if (now !== undefined) vi.setSystemTime(now)
}

export function advanceTimersBy(ms: number) {
  act(() => {
    vi.advanceTimersByTime(ms)
  })
}

export function resetTestTimers() {
  vi.useRealTimers()
}
