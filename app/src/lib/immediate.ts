type ImmediateTask = () => void

const pendingImmediates = new Map<number, ImmediateTask>()
let nextImmediateId = 1
let immediateChannel: MessageChannel | null = null

function closeImmediateChannel(): void {
  if (immediateChannel !== null) {
    immediateChannel.port1.close()
    immediateChannel.port2.close()
    immediateChannel = null
  }
}

function channelForImmediates(): MessageChannel {
  if (immediateChannel === null) {
    immediateChannel = new MessageChannel()
    immediateChannel.port1.onmessage = event => {
      const task = pendingImmediates.get(event.data)
      if (task === undefined) {
        return
      }
      pendingImmediates.delete(event.data)
      try {
        task()
      } finally {
        if (pendingImmediates.size === 0) {
          closeImmediateChannel()
        }
      }
    }
  }
  return immediateChannel
}

export function scheduleImmediate<TArgs extends unknown[]>(
  callback: (...args: TArgs) => void,
  ...args: TArgs
): number {
  const id = nextImmediateId
  nextImmediateId += 1
  pendingImmediates.set(id, () => callback(...args))
  channelForImmediates().port2.postMessage(id)
  return id
}

export function cancelImmediate(id: number): void {
  pendingImmediates.delete(id)
  if (pendingImmediates.size === 0) {
    closeImmediateChannel()
  }
}
