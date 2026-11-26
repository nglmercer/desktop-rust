export function structuralEquals<T extends object>(
  actual: T,
  expected: T
): boolean {
  return strictDeepEquals(actual, expected)
}

function strictDeepEquals(actual: unknown, expected: unknown): boolean {
  if (Object.is(actual, expected)) {
    return true
  }
  if (
    actual === null ||
    actual === undefined ||
    expected === null ||
    expected === undefined ||
    (typeof actual !== 'object' && typeof expected !== 'object')
  ) {
    return Object.is(actual, expected)
  }
  if (typeof actual !== typeof expected) {
    return false
  }

  const left = actual as Record<string, unknown>
  const right = expected as Record<string, unknown>
  if (left.prototype !== right.prototype) {
    return false
  }
  if (isArgumentsObject(left) !== isArgumentsObject(right)) {
    return false
  }
  if (left instanceof RegExp || right instanceof RegExp) {
    return (
      left instanceof RegExp &&
      right instanceof RegExp &&
      left.source === right.source &&
      left.flags === right.flags
    )
  }
  if (left instanceof Date && right instanceof Date) {
    return left.getTime() === right.getTime()
  }

  const leftBuffer = isBufferLike(left)
  const rightBuffer = isBufferLike(right)
  if (leftBuffer !== rightBuffer) {
    return false
  }
  if (leftBuffer) {
    if (left.length !== right.length) {
      return false
    }
    for (let index = 0; index < (left.length as number); index++) {
      if (left[index] !== right[index]) {
        return false
      }
    }
    return true
  }

  let leftKeys: Array<string>
  let rightKeys: Array<string>
  try {
    leftKeys = Object.keys(left)
    rightKeys = Object.keys(right)
  } catch {
    return false
  }
  if (leftKeys.length !== rightKeys.length) {
    return false
  }
  leftKeys.sort()
  rightKeys.sort()
  for (let index = 0; index < leftKeys.length; index++) {
    if (leftKeys[index] !== rightKeys[index]) {
      return false
    }
  }
  for (const key of leftKeys) {
    if (!strictDeepEquals(left[key], right[key])) {
      return false
    }
  }
  return true
}

function isArgumentsObject(value: unknown): boolean {
  return Object.prototype.toString.call(value) === '[object Arguments]'
}

function isBufferLike(value: Record<string, unknown>): boolean {
  if (typeof value.length !== 'number') {
    return false
  }
  if (typeof value.copy !== 'function' || typeof value.slice !== 'function') {
    return false
  }
  return value.length === 0 || typeof value[0] === 'number'
}

/**
 * Performs a shallow equality comparison on the two objects, iterating over
 * their keys (non-recursively) and compares their values.
 *
 * This method is functionally identical to that of React's shallowCompare
 * function and is intended to be used where we need to test for the same
 * kind of equality comparisons that a PureComponent performs.
 *
 * Note that for Arrays and primitive types this method will follow the same
 * semantics as Object.is, see https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Object/is
 */
export function shallowEquals(x: any, y: any) {
  if (Object.is(x, y)) {
    return true
  }

  // After this we know that neither side is null or undefined
  if (
    x === null ||
    y === null ||
    typeof x !== 'object' ||
    typeof y !== 'object'
  ) {
    return false
  }

  const xKeys = Object.keys(x)
  const yKeys = Object.keys(y)

  if (xKeys.length !== yKeys.length) {
    return false
  }

  for (let i = 0; i < xKeys.length; i++) {
    const key = xKeys[i]
    if (!Object.hasOwn(y, key) || !Object.is(x[key], y[key])) {
      return false
    }
  }

  return true
}

/**
 * Compares two arrays for element reference equality.
 *
 * Two arrays are considered equal if they either contain the
 * exact same elements in the same order (reference equality)
 * if they're both empty, or if they are the exact same object
 */
export function arrayEquals<T>(x: ReadonlyArray<T>, y: ReadonlyArray<T>) {
  if (x === y) {
    return true
  }

  if (x.length !== y.length) {
    return false
  }

  for (let i = 0; i < x.length; i++) {
    if (x[i] !== y[i]) {
      return false
    }
  }

  return true
}

/**
 * Compares two maps for key reference equality.
 *
 * Two maps are considered equal if all their keys coincide, if they're
 * both empty or if they're the same object.
 */
export function mapKeysEqual<T>(x: Map<T, unknown>, y: Map<T, unknown>) {
  if (x === y) {
    return true
  }

  if (x.size !== y.size) {
    return false
  }

  for (const key of x.keys()) {
    if (!y.has(key)) {
      return false
    }
  }

  return true
}
