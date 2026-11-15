/**
 * A `node:test` surface on top of Vitest, so the remaining `node:test` suites
 * can move to the runner that owns the Rust backend harness without rewriting
 * every mock (see rust/ROADMAP.md, R2). Import `describe`, `it`, `mock`, the
 * hooks and `TestContext` from here instead of `node:test`.
 *
 * Covered: `t.mock.method|fn|timers`, `t.after`, `t.test`, `mock.method|fn|
 * restoreAll|timers`, and the per-mock `.mock` context (`calls`, `callCount`,
 * `mockImplementation`, `mockImplementationOnce`, `resetCalls`, `restore`).
 * `mock.module` is not covered; suites using it need a Vitest `vi.mock`.
 */
import type {
  TestContext as NodeTestContext,
  mock as nodeMock,
} from 'node:test'
import {
  afterAll,
  afterEach as vitestAfterEach,
  beforeAll,
  beforeEach as vitestBeforeEach,
  describe as vitestDescribe,
  it as vitestIt,
  onTestFinished,
  vi,
} from 'vitest'

export interface IMockCall<F extends (...args: any[]) => any> {
  readonly arguments: Parameters<F>
  readonly result: ReturnType<F> | undefined
  readonly error: unknown
  readonly this: unknown
}

export interface IMockFunctionContext<F extends (...args: any[]) => any> {
  readonly calls: Array<IMockCall<F>>
  callCount(): number
  mockImplementation(implementation: F): void
  mockImplementationOnce(implementation: F, onCall?: number): void
  resetCalls(): void
  restore(): void
}

export type Mocked<F extends (...args: any[]) => any> = F & {
  mock: IMockFunctionContext<F>
}

type AnyFunction = (...args: any[]) => any

function createMock<F extends AnyFunction>(
  initial: F,
  restore: () => void
): Mocked<F> {
  let implementation: F = initial
  const once = new Map<number, F>()
  const calls: Array<IMockCall<F>> = []
  const mocked = function (this: unknown, ...args: Parameters<F>) {
    const index = calls.length
    const run = once.get(index) ?? implementation
    once.delete(index)
    const call = {
      arguments: args,
      result: undefined as ReturnType<F> | undefined,
      error: undefined as unknown,
      this: this,
    }
    calls.push(call)
    try {
      const result = run.apply(this, args)
      call.result = result
      return result
    } catch (error) {
      call.error = error
      throw error
    }
  } as Mocked<F>
  mocked.mock = {
    calls,
    callCount: () => calls.length,
    mockImplementation: next => {
      implementation = next
    },
    mockImplementationOnce: (next, onCall) => {
      once.set(onCall ?? calls.length, next)
    },
    resetCalls: () => {
      calls.length = 0
    },
    restore,
  }
  return mocked
}

type TimerApi = 'setTimeout' | 'setInterval' | 'setImmediate' | 'Date'

class MockTimers {
  private enabled = false

  public enable(options?: {
    apis?: ReadonlyArray<TimerApi>
    now?: number | Date
  }) {
    const toFake = (options?.apis ?? ['setTimeout', 'setInterval', 'Date']).map(
      api => api
    )
    vi.useFakeTimers({
      toFake: toFake as Parameters<typeof vi.useFakeTimers>[0] extends infer O
        ? O extends { toFake?: infer T }
          ? NonNullable<T>
          : never
        : never,
      ...(options?.now === undefined ? {} : { now: options.now }),
    })
    this.enabled = true
  }

  public setTime(milliseconds: number) {
    vi.setSystemTime(milliseconds)
  }

  public tick(milliseconds: number = 1) {
    vi.advanceTimersByTime(milliseconds)
  }

  public runAll() {
    vi.runAllTimers()
  }

  public reset() {
    if (this.enabled) {
      vi.useRealTimers()
      this.enabled = false
    }
  }

  public [Symbol.dispose]() {
    this.reset()
  }
}

/** The mock tracker `t.mock` and the module-level `mock`. */
export class MockTracker {
  private readonly restores = new Set<() => void>()
  public readonly timers = new MockTimers()

  public method<T extends object, K extends keyof T & string>(
    object: T,
    name: K,
    implementation?: T[K] extends AnyFunction ? T[K] : never,
    options?: { getter?: boolean; setter?: boolean }
  ): Mocked<T[K] extends AnyFunction ? T[K] : AnyFunction> {
    const descriptorOwner = (() => {
      let owner: object | null = object
      while (owner !== null && !Object.hasOwn(owner, name)) {
        owner = Object.getPrototypeOf(owner)
      }
      return owner
    })()
    const descriptor =
      descriptorOwner === null
        ? undefined
        : Object.getOwnPropertyDescriptor(descriptorOwner, name)
    const hadOwn = Object.hasOwn(object, name)

    if (options?.getter || options?.setter) {
      throw new Error('Mocking accessors is not supported by node-test-compat')
    }
    const original = (object as any)[name] as AnyFunction
    const restore = () => {
      this.restores.delete(restore)
      if (hadOwn && descriptor !== undefined) {
        Object.defineProperty(object, name, descriptor)
      } else {
        delete (object as any)[name]
      }
    }
    const mocked = createMock(
      (implementation ?? original) as AnyFunction,
      restore
    )
    Object.defineProperty(object, name, {
      configurable: true,
      enumerable: descriptor?.enumerable ?? true,
      writable: true,
      value: mocked,
    })
    this.restores.add(restore)
    return mocked as any
  }

  public fn<F extends AnyFunction = () => undefined>(
    implementation?: F
  ): Mocked<F> {
    const restore = () => {
      this.restores.delete(restore)
    }
    this.restores.add(restore)
    return createMock((implementation ?? (() => undefined)) as F, restore)
  }

  public restoreAll() {
    for (const restore of [...this.restores]) {
      restore()
    }
  }

  public reset() {
    this.restoreAll()
    this.timers.reset()
  }
}

/**
 * Typed as node's own context so helpers and suites keep their exact types;
 * at runtime it provides the subset listed at the top of this file.
 */
export type TestContext = NodeTestContext

interface ICompatContext {
  readonly mock: MockTracker
  readonly name: string
  readonly signal: AbortSignal
  after(fn: () => void | Promise<void>): void
  diagnostic(message: string): void
  test(
    name: string,
    fn: (t: TestContext) => void | Promise<void>
  ): Promise<void>
}

function createContext(name: string, nested = false): TestContext {
  const mock = new MockTracker()
  const controller = new AbortController()
  const cleanups: Array<() => void | Promise<void>> = []
  // Cleanups run last-in-first-out, like node's `t.after`.
  const finish = async () => {
    for (const cleanup of cleanups.reverse()) {
      await cleanup()
    }
    mock.reset()
    controller.abort()
  }
  if (!nested) {
    onTestFinished(finish)
  }
  const context: ICompatContext = {
    mock,
    name,
    signal: controller.signal,
    after: fn => {
      cleanups.push(fn)
    },
    diagnostic: () => {},
    // A nested subtest runs to completion inside its parent and cleans up
    // before the parent continues.
    test: async (childName, fn) => {
      const child = createContext(`${name} > ${childName}`, true)
      try {
        await fn(child)
      } finally {
        await (child as unknown as { __finish(): Promise<void> }).__finish()
      }
    },
  }
  if (nested) {
    Object.defineProperty(context, '__finish', { value: finish })
  }
  return context as unknown as TestContext
}

type TestFunction = (t: TestContext) => void | Promise<void>
interface ITestOptions {
  readonly skip?: boolean | string
  readonly todo?: boolean | string
  readonly timeout?: number
}

function wrap(fn: TestFunction, name: string) {
  return async () => fn(createContext(name))
}

function define(
  runner: typeof vitestIt,
  name: string,
  optionsOrFn: ITestOptions | TestFunction,
  maybeFn?: TestFunction
) {
  const options = typeof optionsOrFn === 'function' ? {} : optionsOrFn
  const fn = typeof optionsOrFn === 'function' ? optionsOrFn : maybeFn
  if (fn === undefined) {
    return runner.todo(name)
  }
  if (options.todo) {
    return runner.todo(name)
  }
  return (options.skip ? runner.skip : runner)(
    name,
    options.timeout === undefined ? {} : { timeout: options.timeout },
    wrap(fn, name)
  )
}

export const it = Object.assign(
  (
    name: string,
    optionsOrFn: ITestOptions | TestFunction,
    maybeFn?: TestFunction
  ) => define(vitestIt, name, optionsOrFn, maybeFn),
  {
    skip: (name: string, fn?: TestFunction) =>
      fn === undefined
        ? vitestIt.skip(name)
        : vitestIt.skip(name, wrap(fn, name)),
    todo: (name: string) => vitestIt.todo(name),
  }
)
export const test = it
export const describe = vitestDescribe

export const before = beforeAll
export const after = afterAll
export const beforeEach = vitestBeforeEach
export const afterEach = vitestAfterEach

/** Module-level tracker, restored after every test like node's global `mock`. */
export const mock = new MockTracker() as unknown as typeof nodeMock
const moduleMock = mock as unknown as MockTracker
vitestAfterEach(() => {
  moduleMock.reset()
})
