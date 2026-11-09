// `async_hooks` for the webview build. There is no async-context tracking in
// a browser, so the store is only visible synchronously inside `run`.
export function createHook(_callbacks?: unknown) {
  const hook = { enable: () => hook, disable: () => hook }
  return hook
}
export function executionAsyncId() {
  return 0
}
export function triggerAsyncId() {
  return 0
}

export class AsyncLocalStorage<T> {
  private store: T | undefined = undefined

  public getStore() {
    return this.store
  }
  public run<R>(store: T, callback: (...args: any[]) => R, ...args: any[]): R {
    const previous = this.store
    this.store = store
    try {
      return callback(...args)
    } finally {
      this.store = previous
    }
  }
  public exit<R>(callback: (...args: any[]) => R, ...args: any[]): R {
    return this.run(undefined as T, callback, ...args)
  }
  public enterWith(store: T) {
    this.store = store
  }
  public disable() {
    this.store = undefined
  }
}

export class AsyncResource {
  public runInAsyncScope<R>(fn: (...args: any[]) => R, thisArg?: unknown, ...args: any[]): R {
    return fn.apply(thisArg, args)
  }
  public emitDestroy() {
    return this
  }
}

export default { createHook, executionAsyncId, triggerAsyncId, AsyncLocalStorage, AsyncResource }
