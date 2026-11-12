/** Serialize selection effects and ignore completions superseded by newer choices. */
export class RepositorySelectionQueue {
  private generation = 0
  private tail: Promise<void> = Promise.resolve()

  public constructor(
    private readonly apply: (path: string | null) => Promise<void>
  ) {}

  public select(path: string | null): Promise<boolean> {
    const generation = ++this.generation
    const result = this.tail.then(async () => {
      if (generation !== this.generation) return false
      try {
        await this.apply(path)
      } catch (error) {
        if (generation !== this.generation) return false
        // The server may have completed before a transport failure. Clear the
        // binding so a previous renderer selection cannot target that root.
        try {
          await this.apply(null)
        } catch {
          /* report the original error */
        }
        throw error
      }
      return generation === this.generation
    })
    this.tail = result.then(
      () => undefined,
      () => undefined
    )
    return result
  }
}
