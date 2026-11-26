export class LruCache<K, V> {
  private readonly entries = new Map<K, V>()
  private readonly maxSize: number

  public constructor({ maxSize }: { readonly maxSize: number }) {
    if (!Number.isInteger(maxSize) || maxSize < 1) {
      throw new Error('LRU cache size must be a positive integer')
    }
    this.maxSize = maxSize
  }

  public get size() {
    return this.entries.size
  }

  public has(key: K) {
    return this.entries.has(key)
  }

  public get(key: K): V | undefined {
    if (!this.entries.has(key)) return undefined
    const value = this.entries.get(key) as V
    this.entries.delete(key)
    this.entries.set(key, value)
    return value
  }

  public set(key: K, value: V) {
    if (this.entries.has(key)) {
      this.entries.delete(key)
    } else if (this.entries.size >= this.maxSize) {
      const oldest = this.entries.keys().next()
      if (!oldest.done) this.entries.delete(oldest.value)
    }
    this.entries.set(key, value)
    return this
  }

  public delete(key: K) {
    return this.entries.delete(key)
  }

  public clear() {
    this.entries.clear()
  }
}

function defaultCacheKey(...args: unknown[]): unknown {
  if (args.length === 0) return '__defaultKey'
  if (args.length === 1) {
    const first = args[0]
    if (
      first === null ||
      first === undefined ||
      (typeof first !== 'function' && typeof first !== 'object')
    ) {
      return first
    }
  }
  return JSON.stringify(args)
}

export function memoize<Arguments extends unknown[], Result>(
  fn: (...args: Arguments) => Result,
  options: {
    readonly maxSize?: number
    readonly cacheKey?: (...args: Arguments) => unknown
  } = {}
): (...args: Arguments) => Result {
  const cache = new LruCache<unknown, Result>({
    maxSize: options.maxSize ?? Number.MAX_SAFE_INTEGER,
  })
  return function (this: unknown, ...args: Arguments): Result {
    const key = options.cacheKey
      ? options.cacheKey(...args)
      : defaultCacheKey(...args)
    if (cache.has(key)) return cache.get(key) as Result
    const result = fn.apply(this, args)
    cache.set(key, result)
    if (
      result !== null &&
      (typeof result === 'object' || typeof result === 'function') &&
      typeof (result as unknown as Promise<Result>).then === 'function'
    ) {
      Promise.resolve(result).catch(() => {
        if (cache.get(key) === result) cache.delete(key)
      })
    }
    return result
  }
}
