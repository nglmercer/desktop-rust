import { Disposable } from 'event-kit'
import { Tailer } from './tailer'
import { forEachLine, type LineStream } from './line-stream'
import { randomUUID } from '../web/random-uuid'
import { join } from 'path'
import { tmpdir } from '../web/os'

/**
 * Get a path to a temp file using the given name. Note that the file itself
 * will not be created.
 */
export const getTempFilePath = (name: string) =>
  join(tmpdir(), `${name}-${randomUUID().replaceAll('-', '').slice(0, 16)}`)

/**
 * Tail the file and call the callback on every line.
 *
 * Note that this will not stop tailing until the returned `Disposable` is
 * disposed of.
 */
export function tailByLine(
  path: string,
  cb: (line: string) => void
): Disposable {
  const tailer = new Tailer(path)

  const onErrorDisposable = tailer.onError(error => {
    log.warn(`Unable to tail path: ${path}`, error)
  })

  const onDataDisposable = tailer.onDataAvailable(stream => {
    forEachLine(stream as unknown as LineStream, line => {
      if (!onDataDisposable.disposed) cb(line)
    })
  })

  tailer.start()

  return new Disposable(() => {
    onDataDisposable.dispose()
    onErrorDisposable.dispose()
    tailer.stop()
  })
}
