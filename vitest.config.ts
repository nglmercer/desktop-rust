import { defineConfig } from 'vitest/config'
import { readFileSync } from 'node:fs'
import { parseEnv } from 'node:util'
import { fileURLToPath } from 'node:url'
import suites from './vitest-suites.json'

// Migrated suites are explicitly routed here by script/test.mts. Do not load
// the application Vite config: it aliases Node modules for the webview.
Object.assign(
  process.env,
  parseEnv(readFileSync(new URL('.test.env', import.meta.url), 'utf8'))
)

// Tests start a real Rust backend on Linux (see app/test/vitest-setup.ts); a
// loaded machine can take a few seconds for a Git round trip through it.
const rustBackendDefault = process.platform === 'linux'
if (process.env.DESKTOP_TEST_RUST_BACKEND === undefined && rustBackendDefault) {
  process.env.DESKTOP_TEST_RUST_BACKEND = '1'
}

export default defineConfig({
  resolve: {
    alias: [
      // Native Node addons are not installed; use the renderer's web shims.
      ...['keytar', 'registry-js'].map(name => ({
        find: new RegExp(`^${name}$`),
        replacement: fileURLToPath(
          new URL(`./app/src/web/${name}.ts`, import.meta.url)
        ),
      })),
      {
        find: /^desktop-notifications$/,
        replacement: fileURLToPath(
          new URL(
            './vendor/desktop-notifications/lib/index.ts',
            import.meta.url
          )
        ),
      },
      // On Windows the installed package resolves its compiled native addon
      // relative to build/index.js. Other platforms can load the TypeScript
      // source directly even when install scripts were disabled.
      ...(process.platform === 'win32'
        ? []
        : [
            {
              find: /^windows-argv-parser$/,
              replacement: fileURLToPath(
                new URL(
                  './vendor/windows-argv-parser/index.ts',
                  import.meta.url
                )
              ),
            },
          ]),
    ],
  },
  test: {
    include: suites,
    maxWorkers: 2,
    testTimeout: 15_000,
    hookTimeout: 60_000,
    environment: 'jsdom',
    environmentOptions: { jsdom: { url: 'http://localhost/' } },
    setupFiles: ['./app/test/vitest-setup.ts'],
  },
})
