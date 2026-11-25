/** Prepare bounded literal-path batches before starting an index mutation. */
export function batchIndexPaths(
  paths: ReadonlyArray<string>
): ReadonlyArray<ReadonlyArray<string>> {
  const batches: Array<Array<string>> = []
  let batch: Array<string> = []
  let size = 0
  const encoder = new TextEncoder()
  for (const path of paths) {
    const pathSize = encoder.encode(JSON.stringify(path)).length + 1
    if (pathSize > 50_000) {
      throw new Error('Index path exceeds the request limit')
    }
    if (batch.length === 4096 || size + pathSize > 50_000) {
      batches.push(batch)
      batch = []
      size = 0
    }
    batch.push(path)
    size += pathSize
  }
  if (batch.length > 0) batches.push(batch)
  return batches
}
