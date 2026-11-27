import { vi } from 'vitest'

/**
 * Transitional method spy helper for tests converted from the node:test mock
 * API. Prefer direct vi.spyOn calls in new tests.
 */
export function mockMethod(
  object: object,
  method: string,
  implementation?: unknown
) {
  const spy = vi.spyOn(
    object as never,
    method as never
  ) as unknown as ReturnType<typeof vi.fn>
  return implementation === undefined
    ? spy
    : spy.mockImplementation(implementation as never)
}
