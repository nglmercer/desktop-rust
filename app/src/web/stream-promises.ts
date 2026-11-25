// `stream/promises` for the webview build. The callback pipeline comes from
// the same Node-compatible stream implementation as `stream` itself.
import {
  finished as finishedCallback,
  pipeline as pipelineCallback,
} from 'stream'

type StreamLike = unknown

export function pipeline(...streams: StreamLike[]): Promise<void> {
  return new Promise((resolve, reject) => {
    ;(pipelineCallback as (...args: unknown[]) => void)(
      ...streams,
      (error: Error | null | undefined) => (error ? reject(error) : resolve())
    )
  })
}

export function finished(stream: StreamLike): Promise<void> {
  return new Promise((resolve, reject) => {
    ;(finishedCallback as (...args: unknown[]) => void)(
      stream,
      (error: Error | null | undefined) => (error ? reject(error) : resolve())
    )
  })
}
