export function limitConcurrency(
  concurrency: number
): <T>(fn: () => Promise<T> | T) => Promise<T> {
  if (!Number.isInteger(concurrency) || concurrency < 1) {
    throw new Error('Concurrency must be a positive integer')
  }
  let active = 0
  const queue: Array<() => void> = []
  const drain = () => {
    while (active < concurrency && queue.length > 0) {
      queue.shift()?.()
    }
  }
  return function limit<T>(fn: () => Promise<T> | T): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      queue.push(() => {
        active++
        Promise.resolve()
          .then(fn)
          .then(
            value => {
              active--
              resolve(value)
              drain()
            },
            error => {
              active--
              reject(error)
              drain()
            }
          )
      })
      drain()
    })
  }
}
