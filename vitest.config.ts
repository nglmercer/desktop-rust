import { defineConfig } from 'vitest/config'
import { readFileSync } from 'node:fs'
import { parseEnv } from 'node:util'
import suites from './vitest-suites.json'

// Migrated suites are explicitly routed here by script/test.mts. Do not load
// the application Vite config: it aliases Node modules for the webview.
Object.assign(
  process.env,
  parseEnv(readFileSync(new URL('.test.env', import.meta.url), 'utf8'))
)

export default defineConfig({
  test: {
    include: suites,
    maxWorkers: 2,
    environment: 'jsdom',
    environmentOptions: { jsdom: { url: 'http://localhost/' } },
    setupFiles: ['./app/test/vitest-setup.ts'],
  },
})
