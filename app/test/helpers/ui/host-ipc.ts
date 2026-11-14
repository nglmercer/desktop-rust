import { hostIpc } from '../../../src/web/host-bridge'

export function captureClipboardWrites() {
  const writes = new Array<string>()
  const previousInvoke = hostIpc.invoke

  hostIpc.invoke = async (channel: string, ...args: any[]) => {
    if (channel === 'write-clipboard-text') {
      writes.push(args[0])
      return
    }

    return previousInvoke.call(hostIpc, channel, ...args)
  }

  return {
    writes,
    restore() {
      hostIpc.invoke = previousInvoke
    },
  }
}
