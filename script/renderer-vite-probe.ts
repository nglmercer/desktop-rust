import { spawn, spawnSync } from 'node:child_process'
import {
  cpSync,
  mkdtempSync,
  mkdirSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { chromium } from 'playwright'

async function stopProcessGroup(child: ReturnType<typeof spawn>) {
  const pid = child.pid
  if (pid === undefined) {
    return
  }
  const signalGroup = (signal: NodeJS.Signals | 0) => {
    try {
      process.kill(-pid, signal)
      return true
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ESRCH') {
        return false
      }
      throw error
    }
  }
  signalGroup('SIGTERM')
  const deadline = Date.now() + 5000
  while (signalGroup(0)) {
    if (Date.now() >= deadline) {
      signalGroup('SIGKILL')
      break
    }
    await new Promise(resolve => setTimeout(resolve, 100))
  }
  const killDeadline = Date.now() + 5000
  while (signalGroup(0)) {
    if (Date.now() >= killDeadline) {
      throw new Error(
        'Electron process group did not exit; profile was retained'
      )
    }
    await new Promise(resolve => setTimeout(resolve, 100))
  }
}

async function main() {
  const base = process.env.DESKTOP_VITE_PROBE_APP
  if (
    process.platform !== 'linux' ||
    base === undefined ||
    !path.isAbsolute(base)
  ) {
    throw new Error(
      'Set DESKTOP_VITE_PROBE_APP to an absolute Linux installed app resources/app directory'
    )
  }

  const root = path.resolve(__dirname, '..')
  const directory = mkdtempSync(path.join(tmpdir(), 'desktop-vite-renderer-'))
  const runtimeDirectory = path.join(directory, 'runtime')
  mkdirSync(runtimeDirectory, { mode: 0o700 })
  const app = path.join(directory, 'app')
  const inherited = Object.fromEntries(
    Object.entries(process.env).filter(
      ([key]) =>
        !key.startsWith('GIT_') &&
        ![
          'DBUS_SESSION_BUS_ADDRESS',
          'DBUS_STARTER_ADDRESS',
          'DBUS_STARTER_BUS_TYPE',
          'GNOME_KEYRING_CONTROL',
          'SSH_AUTH_SOCK',
          'SSH_AGENT_PID',
          'SSH_ASKPASS',
          'WAYLAND_DISPLAY',
        ].includes(key)
    )
  )
  const env = {
    ...inherited,
    DESKTOP_RENDERER_FS_ADMIN_NODE: path.join(base, 'fs_admin.node'),
    DESKTOP_RENDERER_KEYTAR_NODE: path.join(base, 'keytar.node'),
    GIT_CONFIG_NOSYSTEM: '1',
    GIT_CONFIG_GLOBAL: path.join(directory, 'gitconfig'),
    GIT_CONFIG_COUNT: '1',
    GIT_CONFIG_KEY_0: 'core.hooksPath',
    GIT_CONFIG_VALUE_0: '/dev/null',
    DBUS_SESSION_BUS_ADDRESS: `unix:path=${path.join(directory, 'no-dbus')}`,
    HOME: directory,
    XDG_CONFIG_HOME: path.join(directory, 'config'),
    XDG_DATA_HOME: path.join(directory, 'data'),
    XDG_CACHE_HOME: path.join(directory, 'cache'),
    XDG_RUNTIME_DIR: runtimeDirectory,
  }
  let child: ReturnType<typeof spawn> | undefined

  try {
    for (const [config, output] of [
      ['vite.main.config.ts', path.join(directory, 'main')],
      [
        'vite.renderer.experimental.config.ts',
        path.join(directory, 'renderer'),
      ],
    ]) {
      const result = spawnSync(
        process.execPath,
        [
          'node_modules/vite/bin/vite.js',
          'build',
          '-c',
          config,
          '--outDir',
          output,
        ],
        {
          cwd: root,
          env,
          stdio: 'inherit',
        }
      )
      if (result.status !== 0) {
        throw new Error(`Build failed: ${config}`)
      }
    }
    cpSync(base, app, { recursive: true, dereference: true })
    cpSync(path.join(directory, 'renderer'), app, {
      recursive: true,
    })
    cpSync(path.join(directory, 'main/main.js'), path.join(app, 'main.js'))
    child = spawn(
      path.join(root, 'node_modules/electron/dist/electron'),
      [
        '--no-sandbox',
        '--disable-gpu',
        '--ozone-platform=x11',
        '--remote-debugging-port=0',
        `--user-data-dir=${path.join(directory, 'userdata')}`,
        app,
      ],
      {
        env: {
          ...env,
          ELECTRON_DISABLE_SECURITY_WARNINGS: 'true',
        },
        stdio: ['ignore', 'pipe', 'pipe'],
        detached: true,
      }
    )
    let output = ''
    child.stdout?.on('data', chunk => {
      output += chunk.toString()
    })
    child.stderr?.on('data', chunk => {
      output += chunk.toString()
    })
    const deadline = Date.now() + 30000
    let endpoint: string | undefined
    while (Date.now() < deadline && endpoint === undefined) {
      endpoint = output.match(/DevTools listening on (ws:\/\/[^\s]+)/)?.[1]
      if (endpoint === undefined) {
        await new Promise(resolve => setTimeout(resolve, 100))
      }
    }
    if (endpoint === undefined) {
      throw new Error(`Electron did not start: ${output.slice(-4000)}`)
    }
    const browser = await chromium.connectOverCDP(endpoint)
    try {
      let page = browser
        .contexts()
        .flatMap(context => context.pages())
        .find(candidate => candidate.url().startsWith('file:'))
      while (Date.now() < deadline && page === undefined) {
        await new Promise(resolve => setTimeout(resolve, 100))
        page = browser
          .contexts()
          .flatMap(context => context.pages())
          .find(candidate => candidate.url().startsWith('file:'))
      }
      if (page === undefined) {
        throw new Error('Electron did not open its window')
      }
      const errors: string[] = []
      page.on('pageerror', error => errors.push(String(error)))
      page.on('console', message => {
        if (message.type() === 'error') {
          errors.push(message.text())
        }
      })
      await page.getByText('Skip this step').click()
      await page.getByLabel('Name', { exact: true }).fill('Integration Test')
      await page
        .getByLabel('Email', { exact: true })
        .fill('test@example.invalid')
      await page.getByRole('button', { name: 'Finish' }).click()
      await page
        .getByText('Create a New Repository on your local drive', {
          exact: false,
        })
        .click()
      await page.getByPlaceholder('repository name').fill('vite-repo')
      await page.getByPlaceholder('repository path').fill(directory)
      await page.getByRole('button', { name: /Create repository/i }).click()
      await page.getByText('Current repository').waitFor()
      const repository = path.join(directory, 'vite-repo')
      writeFileSync(
        path.join(repository, 'proof.txt'),
        'Vite renderer verified\n'
      )
      await page.reload()
      await page
        .getByText('proof.txt', { exact: true })
        .first()
        .waitFor({ timeout: 20000 })
      await page.getByLabel('Commit summary').fill('Vite renderer test')
      await page
        .getByRole('button', { name: /Commit/ })
        .last()
        .click()
      await page
        .getByText('Vite renderer test', { exact: true })
        .waitFor({ timeout: 20000 })
      const git = spawnSync(
        'git',
        ['-C', repository, 'show', 'HEAD:proof.txt'],
        { encoding: 'utf8', env }
      )
      if (git.status !== 0 || git.stdout !== 'Vite renderer verified\n') {
        throw new Error('Git commit did not contain the expected file')
      }
      const chunk = readdirSync(path.join(directory, 'renderer')).find(name =>
        /^ffiRuntimeHost-.*\.js$/.test(name)
      )
      if (chunk === undefined) {
        throw new Error('Missing FFI runtime chunk')
      }
      const loaded = await page.evaluate(
        name => Object.keys(require(`./${name}`)),
        chunk
      )
      if (!loaded.includes('FfiRuntimeHost')) {
        throw new Error('The relocated FFI runtime did not load')
      }
      const koffiLoaded = await page.evaluate(() =>
        Boolean(require.cache[require.resolve('./native/koffi/index.js')])
      )
      if (!koffiLoaded) {
        throw new Error('The relocated Koffi native package did not load')
      }
      if (errors.length > 0) {
        throw new Error(`Renderer errors: ${errors.join('\n')}`)
      }
      console.log(
        'Vite renderer created a repository, committed a file, and loaded its relocated FFI chunk'
      )
    } finally {
      await browser.close()
    }
  } finally {
    if (child !== undefined) {
      await stopProcessGroup(child)
    }
    rmSync(directory, { recursive: true, force: true })
  }
}

main().catch(error => {
  console.error(error)
  process.exitCode = 1
})
