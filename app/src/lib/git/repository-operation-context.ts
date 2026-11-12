import { createHook } from 'node:async_hooks'

/**
 * Async operation context using public async-hooks callbacks.
 *
 * Electron renderer realms cannot use Node's AsyncContextFrame-backed
 * AsyncLocalStorage: getStore can fatally crash V8 across the renderer realm.
 * Keep context in JavaScript and propagate it through tracked async resources.
 */
export function createRepositoryOperationContext<T>() {
  const resources = new Map<number, T>()
  const stack: Array<T | undefined> = []
  let current: T | undefined

  const hook = createHook({
    init: asyncId => {
      if (current !== undefined) {
        resources.set(asyncId, current)
      }
    },
    before: asyncId => {
      stack.push(current)
      current = resources.get(asyncId)
    },
    after: () => {
      current = stack.pop()
    },
    destroy: asyncId => {
      resources.delete(asyncId)
    },
  })
  const supportsAsyncPropagation =
    (hook as typeof hook & { readonly __desktopNoAsyncPropagation?: boolean })
      .__desktopNoAsyncPropagation !== true
  hook.enable()

  const run = <TResult>(
    value: T | undefined,
    operation: () => TResult
  ): TResult => {
    const previous = current
    current = value
    try {
      return operation()
    } finally {
      current = previous
    }
  }

  return {
    getStore: () => current,
    supportsAsyncPropagation,
    run,
    exit: <TResult>(operation: () => TResult) => run(undefined, operation),
  }
}
