import type {
  ITerminalOptions,
  ITerminalInitOnlyOptions,
  Terminal as XTermTerminal,
} from '@xterm/xterm'
import React from 'react'
import { getMonospaceFontFamily } from './get-monospace-font-family'
import { TerminalOutput } from '../lib/git'

export const defaultTerminalOptions: Readonly<ITerminalOptions> = {
  convertEol: true,
  fontFamily: getMonospaceFontFamily(),
  fontSize: 12,
  screenReaderMode: true,
}

export type TerminalProps = ITerminalOptions &
  ITerminalInitOnlyOptions & {
    readonly terminalOutput?: TerminalOutput
    readonly hideCursor?: boolean
  }

/**
 * Load xterm on first use, in its own chunk. It ships a UMD bundle, so Node
 * resolves its classes through `default`.
 */
async function loadXTermTerminal(): Promise<typeof XTermTerminal> {
  const module = await import('@xterm/xterm')
  return (
    module.Terminal ??
    (module as unknown as { default: typeof module }).default.Terminal
  )
}

export class Terminal extends React.Component<TerminalProps> {
  private terminalRef = React.createRef<HTMLDivElement>()
  private terminal: XTermTerminal | null = null
  private unmounted = false
  /** Output written before xterm finished loading, replayed in order. */
  private pending: TerminalOutput[] = []

  public get Terminal() {
    return this.terminal
  }

  public write(data: TerminalOutput) {
    if (this.terminal === null) {
      this.pending.push(data)
      return
    }
    if (Array.isArray(data)) {
      data.forEach(chunk => this.terminal?.write(chunk))
    } else {
      this.terminal.write(data)
    }
  }

  public componentWillUnmount(): void {
    this.unmounted = true
    this.pending = []
    this.terminal?.dispose()
  }

  public componentDidMount() {
    void this.start()
  }

  private async start() {
    const { terminalOutput, hideCursor, ...initOpts } = this.props
    const XTerm = await loadXTermTerminal()
    if (this.unmounted) {
      return
    }

    const terminal = new XTerm({
      ...defaultTerminalOptions,
      ...initOpts,

      rows: this.props.rows ?? 20,
      cols: this.props.cols ?? 80,
    })

    terminal.attachCustomKeyEventHandler((key: KeyboardEvent) => {
      if (key.key === 'Tab') {
        // We don't want to handle tab key events in the terminal as it
        // breaks tab navigation in the app. The terminal is read only and
        // doesn't support tab input, so we can safely ignore it.
        return false
      }
      return true
    })

    if (this.terminalRef.current) {
      terminal.open(this.terminalRef.current)
      this.terminal = terminal

      if (hideCursor !== false) {
        terminal.write('\x1b[?25l') // hide cursor
        if (terminalOutput) {
          this.write(terminalOutput)
        }
      }
    } else {
      this.terminal = terminal
    }

    const pending = this.pending
    this.pending = []
    for (const data of pending) {
      this.write(data)
    }
  }

  public render() {
    return <div ref={this.terminalRef}></div>
  }
}
