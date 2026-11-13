import * as React from 'react'
import { spawn, type ChildProcess } from 'node:child_process'
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { createInterface } from 'node:readline'
import { createRequire } from 'node:module'
import { afterEach, describe, expect, it, vi } from 'vitest'

const route = vi.hoisted(() => ({
  client: undefined as
    | import('../../src/backend/client').BackendClient
    | undefined,
}))
vi.mock('../../src/backend/client', async importOriginal => ({
  ...(await importOriginal<typeof import('../../src/backend/client')>()),
  backend: () => {
    if (!route.client) {
      throw new Error('Bridge is not connected')
    }
    return route.client
  },
}))
vi.mock('electron', () => ({
  ipcRenderer: { send: vi.fn(), on: vi.fn(), invoke: vi.fn() },
  shell: { openPath: vi.fn(async () => '') },
}))

import { BackendClient } from '../../src/backend/client'
import { CloneRepository } from '../../src/ui/clone-repository/clone-repository'
import { CloneRepositoryTab } from '../../src/models/clone-repository-tab'
import { setDefaultDir } from '../../src/ui/lib/default-dir'
import type { Dispatcher } from '../../src/ui/dispatcher'
import { fireEvent, render, screen, waitFor } from '../helpers/ui/vitest-render'

const JSDOM = createRequire(import.meta.url)('jsdom').JSDOM
let server: ChildProcess | undefined
let directory: string | undefined
let remoteDom:
  | { window: Window & typeof globalThis & { close(): void } }
  | undefined

async function bounded<T>(promise: Promise<T>, ms = 15_000): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_, reject) => {
        timer = setTimeout(
          () => reject(new Error('Bridge operation timed out')),
          ms
        )
      }),
    ])
  } finally {
    if (timer !== undefined) {
      clearTimeout(timer)
    }
  }
}

afterEach(async () => {
  route.client?.close()
  route.client = undefined
  remoteDom?.window.close()
  remoteDom = undefined
  if (server && server.exitCode === null && server.signalCode === null) {
    const child = server
    const stopped = new Promise<void>(resolve =>
      child.once('close', () => resolve())
    )
    child.kill('SIGINT')
    try {
      await bounded(stopped, 5_000)
    } catch {
      child.kill('SIGKILL')
      await bounded(stopped, 5_000)
    }
  }
  server = undefined
  vi.unstubAllGlobals()
  sessionStorage.removeItem('desktopBridgeToken')
  if (directory) {
    await rm(directory, { recursive: true, force: true })
  }
  directory = undefined
})

describe.skipIf(
  process.env.DESKTOP_TEST_BRIDGE !== '1' || process.platform === 'win32'
)('clone dialog with real Rust WebSocket', () => {
  it('validates real directories and dispatches an ordinary clone', async () => {
    directory = await mkdtemp(path.join(tmpdir(), 'desktop-clone-ui-bridge-'))
    const env = Object.fromEntries(
      Object.entries(process.env).filter(([key]) => !key.startsWith('GIT_'))
    )
    server = spawn(
      path.resolve('rust/target/debug/desktop-rs'),
      [
        '--config',
        path.join(directory, 'registry.json'),
        'serve',
        '--empty',
        '--port',
        '0',
      ],
      {
        env: {
          ...env,
          HOME: directory,
          GIT_CONFIG_NOSYSTEM: '1',
          GIT_CONFIG_GLOBAL: path.join(directory, 'global-config'),
        },
        stdio: ['ignore', 'ignore', 'pipe'],
      }
    )
    const lines = createInterface({ input: server.stderr! })
    const line = await bounded(
      new Promise<string>((resolve, reject) => {
        lines.once('line', resolve)
        server!.once('error', reject)
        server!.once('exit', code =>
          reject(new Error(`bridge exited: ${code}`))
        )
      })
    )
    expect(line.startsWith('Open ')).toBe(true)
    const address = new URL(line.slice(5))
    const token = new URLSearchParams(address.hash.slice(1)).get('token')
    expect(token).toBeTruthy()
    remoteDom = new JSDOM('', { url: address.href })
    const RemoteSocket = remoteDom!.window.WebSocket
    const wsUrl = `ws://${address.host}/ws`
    class BoundSocket extends RemoteSocket {
      public constructor(_url: string | URL, protocols?: string | string[]) {
        super(wsUrl, protocols)
      }
    }
    vi.stubGlobal('WebSocket', BoundSocket)
    sessionStorage.setItem('desktopBridgeToken', token!)
    const client = new BackendClient()
    route.client = client
    await bounded(client.waitReady())

    const empty = path.join(directory, 'empty')
    const nonempty = path.join(directory, 'nonempty')
    const file = path.join(directory, 'file')
    await mkdir(empty)
    await mkdir(nonempty)
    await writeFile(path.join(nonempty, 'existing'), 'leave unchanged')
    await writeFile(file, 'existing')
    setDefaultDir(directory)
    const clone = vi.fn()
    render(
      <CloneRepository
        dispatcher={{ clone, closeFoldout: vi.fn() } as unknown as Dispatcher}
        onDismissed={vi.fn()}
        accounts={[]}
        initialURL="https://example.com/owner/repository.git"
        selectedTab={CloneRepositoryTab.Generic}
        selectedAccount={null}
        onTabSelected={vi.fn()}
        apiRepositories={new Map()}
        onRefreshRepositories={vi.fn()}
        isTopMost={true}
      />
    )
    const input = screen.getByRole('textbox', {
      name: /Local path/i,
      hidden: true,
    })
    const button = screen.getByRole('button', { name: 'Clone', hidden: true })
    await waitFor(() =>
      expect((input as HTMLInputElement).value).toBe(
        path.join(directory!, 'repository')
      )
    )
    fireEvent.change(input, { target: { value: nonempty } })
    await waitFor(() =>
      expect(
        screen.getByText(
          'This folder contains files. Git can only clone to empty folders.'
        )
      ).toBeTruthy()
    )
    expect(button.getAttribute('aria-disabled')).toBe('true')
    fireEvent.change(input, { target: { value: file } })
    await waitFor(() =>
      expect(
        screen.getByText(
          'There is already a file with this name. Git can only clone to a folder.'
        )
      ).toBeTruthy()
    )
    fireEvent.change(input, { target: { value: empty } })
    await waitFor(() => expect(button.getAttribute('aria-disabled')).toBeNull())
    fireEvent.click(button)
    await waitFor(() =>
      expect(clone).toHaveBeenCalledWith(
        'https://example.com/owner/repository.git',
        empty,
        { defaultBranch: undefined }
      )
    )
  }, 60_000)
})
