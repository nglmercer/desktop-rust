// `async_hooks` for the webview build. There is no async-context tracking in
// a browser, so the store is only visible synchronously inside `run`.
export function createHook(_callbacks?: unknown) {
  const hook = {
    __desktopNoAsyncPropagation: true,
    enable: () => hook,
    disable: () => hook,
  }
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
  public run<R, A extends unknown[]>(
    store: T,
    callback: (...args: A) => R,
    ...args: A
  ): R {
    const previous = this.store
    this.store = store
    try {
      return callback(...args)
    } finally {
      this.store = previous
    }
  }
  public exit<R, A extends unknown[]>(
    callback: (...args: A) => R,
    ...args: A
  ): R {
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
  public runInAsyncScope<R, A extends unknown[]>(
    fn: (...args: A) => R,
    thisArg?: unknown,
    ...args: A
  ): R {
    return fn.apply(thisArg, args)
  }
  public emitDestroy() {
    return this
  }
}

export default {
  createHook,
  executionAsyncId,
  triggerAsyncId,
  AsyncLocalStorage,
  AsyncResource,
}
