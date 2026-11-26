type Equality = (newArg: unknown, lastArg: unknown, index: number) => boolean

const simpleIsEqual: Equality = (left, right) => left === right

export function memoizeOne<Arguments extends unknown[], Result>(
  resultFn: (...args: Arguments) => Result,
  isEqual: Equality = simpleIsEqual
): (...args: Arguments) => Result {
  let lastThis: unknown
  let lastArgs: unknown[] = []
  let lastResult: Result
  let calledOnce = false

  return function (this: unknown, ...newArgs: Arguments): Result {
    if (
      calledOnce &&
      lastThis === this &&
      newArgs.length === lastArgs.length &&
      newArgs.every((newArg, index) => isEqual(newArg, lastArgs[index], index))
    ) {
      return lastResult
    }

    lastResult = resultFn.apply(this, newArgs)
    calledOnce = true
    lastThis = this
    lastArgs = newArgs
    return lastResult
  }
}
