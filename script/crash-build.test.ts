// @vitest-environment node
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { describe, it } from 'vitest'

describe('Vite Electron crash window build', () => {
  for (const mode of ['development', 'production'] as const) {
    it(`builds a standalone ${mode} crash window without deleting sibling bundles`, () => {
      const output = mkdtempSync(path.join(tmpdir(), 'desktop-crash-build-'))
      try {
        writeFileSync(path.join(output, 'other-bundle.js'), 'preserve')
        const build = spawnSync(
          process.execPath,
          [
            'node_modules/vite/bin/vite.js',
            'build',
            '-c',
            'vite.crash.config.ts',
            '--outDir',
            output,
          ],
          {
            encoding: 'utf8',
            timeout: 60_000,
            env: { ...process.env, NODE_ENV: mode, RELEASE_CHANNEL: mode },
          }
        )
        assert.equal(build.status, 0, build.stderr)
        assert.equal(
          readFileSync(path.join(output, 'other-bundle.js'), 'utf8'),
          'preserve'
        )
        const html = readFileSync(path.join(output, 'crash.html'), 'utf8')
        assert.match(html, /href="\.\/crash\.css"/)
        assert.match(html, /src="\.\/crash\.js"/)
        assert.ok(readFileSync(path.join(output, 'crash.css')).length > 0)
        assert.ok(readFileSync(path.join(output, 'crash.js.map')).length > 0)

        // A Linux host with Xvfb and Electron can exercise the actual
        // node-integrated file:// renderer and its crash IPC contract.
        if (process.env.DESKTOP_TEST_ELECTRON_CRASH === '1') {
          assert.equal(
            process.platform,
            'linux',
            'Electron smoke requires Linux and Xvfb'
          )
          const main = path.join(output, 'smoke.cjs')
          writeFileSync(
            main,
            `
const { app, BrowserWindow, ipcMain } = require('electron');
app.whenReady().then(async () => {
  ipcMain.handle('get-current-window-state', () => null);
  const window = new BrowserWindow({ show: false, webPreferences: { nodeIntegration: true, contextIsolation: false } });
  const timer = setTimeout(() => { console.error('crash window timed out'); app.exit(1); }, 10000);
  ipcMain.once('crash-quit', () => { clearTimeout(timer); console.log('CRASH_QUIT'); app.quit(); });
  ipcMain.once('crash-ready', async () => {
    window.webContents.send('error', { type: 'launch', error: { name: 'Error', message: 'smoke failure', stack: 'Error: smoke failure' } });
    await new Promise(resolve => setTimeout(resolve, 100));
    const state = await window.webContents.executeJavaScript("({ title: document.querySelector('h1')?.textContent, error: document.querySelector('pre')?.textContent, button: document.querySelector('button[type=submit]')?.textContent, image: document.querySelector('img.background-graphic-bottom')?.src, css: document.styleSheets.length })");
    console.log('CRASH_STATE:' + JSON.stringify(state));
    await window.webContents.executeJavaScript("document.querySelector('button[type=submit]').click()");
  });
  await window.loadFile(${JSON.stringify(path.join(output, 'crash.html'))});
}).catch(error => { console.error(error); app.exit(1); });
`
          )
          const electron = path.resolve('node_modules/electron/dist/electron')
          const smoke = spawnSync(
            'xvfb-run',
            ['-a', electron, '--no-sandbox', '--disable-gpu', main],
            { encoding: 'utf8', timeout: 30_000 }
          )
          assert.equal(smoke.status, 0, smoke.stderr)
          const match = /^CRASH_STATE:(.+)$/m.exec(smoke.stdout)
          assert.ok(match, smoke.stdout)
          const state = JSON.parse(match[1]) as {
            title: string
            error: string
            button: string
            image: string
            css: number
          }
          assert.equal(state.title, 'GitHub Desktop failed to launch')
          assert.match(state.error, /smoke failure/)
          assert.equal(
            state.button,
            mode === 'development' ? 'Exit' : 'Exit and restart'
          )
          assert.match(
            state.image,
            /\/static\/welcome-illustration-left-bottom\.svg$/
          )
          assert.equal(state.css, 1)
          assert.match(smoke.stdout, /CRASH_QUIT/)
        }
      } finally {
        rmSync(output, { recursive: true, force: true })
      }
    }, 90_000)
  }
})
