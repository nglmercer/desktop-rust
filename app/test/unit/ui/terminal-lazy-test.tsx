import * as React from 'react'
import { describe, expect, it, vi } from 'vitest'
import { render, waitFor } from '../../helpers/ui/vitest-render'

const fake = vi.hoisted(() => ({
  instances: [] as Array<{
    written: string[]
    disposed: boolean
    opened: boolean
  }>,
  release: undefined as undefined | (() => void),
}))

vi.mock('@xterm/xterm', async () => {
  // The module resolves only after the test lets it, as a real chunk would.
  await new Promise<void>(resolve => {
    fake.release = resolve
  })
  class Terminal {
    public readonly state = {
      written: [] as string[],
      disposed: false,
      opened: false,
    }
    public constructor() {
      fake.instances.push(this.state)
    }
    public attachCustomKeyEventHandler() {}
    public open() {
      this.state.opened = true
    }
    public write(data: string | Buffer) {
      this.state.written.push(String(data))
    }
    public dispose() {
      this.state.disposed = true
    }
  }
  return { Terminal }
})

import { Terminal } from '../../../src/ui/terminal'

describe('terminal loads xterm on demand', () => {
  it('replays output written before xterm arrived, in order', async () => {
    const ref = React.createRef<Terminal>()
    render(<Terminal ref={ref} terminalOutput={'first\n'} />)

    // Nothing has been created yet: the chunk has not loaded.
    expect(fake.instances).toHaveLength(0)
    ref.current?.write('second\n')
    ref.current?.write([Buffer.from('third\n'), Buffer.from('fourth\n')])

    await waitFor(() => expect(fake.release).toBeDefined())
    fake.release?.()
    await waitFor(() => expect(fake.instances).toHaveLength(1))

    const [terminal] = fake.instances
    await waitFor(() => expect(terminal.written.length).toBeGreaterThan(3))
    expect(terminal.opened).toBe(true)
    expect(terminal.written).toEqual([
      '\x1b[?25l',
      'first\n',
      'second\n',
      'third\n',
      'fourth\n',
    ])
  })
})
