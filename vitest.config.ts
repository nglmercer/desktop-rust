import { defineConfig } from 'vitest/config'
import { readFileSync } from 'node:fs'
import { parseEnv } from 'node:util'
import { fileURLToPath } from 'node:url'
import { createRequire } from 'node:module'

// Do not load the application Vite config: it aliases Node modules for the webview.

const legacyCryptoPackage = createRequire(
  createRequire(
    createRequire(import.meta.url).resolve('vite-plugin-node-polyfills')
  ).resolve('node-stdlib-browser')
).resolve('crypto-browserify')

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
      // Suites and helpers written for `node:test` run on Vitest through a
      // compatible surface (see app/test/helpers/node-test-compat.ts).
      {
        find: /^node:test$/,
        replacement: fileURLToPath(
          new URL('./app/test/helpers/node-test-compat.ts', import.meta.url)
        ),
      },
      {
        find: /^desktop-dugite\/(.+)$/,
        replacement:
          fileURLToPath(
            new URL('./app/node_modules/dugite/build/lib', import.meta.url)
          ) + '/$1.js',
      },
      // Native Node addons are not installed; use the renderer's web shims.
      ...['registry-js'].map(name => ({
        find: new RegExp(`^${name}$`),
        replacement: fileURLToPath(
          new URL(`./app/src/web/${name}.ts`, import.meta.url)
        ),
      })),
      { find: /^desktop-crypto-impl$/, replacement: legacyCryptoPackage },
      {
        find: /^desktop-notifications$/,
        replacement: fileURLToPath(
          new URL('./app/src/web/desktop-notifications.ts', import.meta.url)
        ),
      },
    ],
  },
  test: {
    include: [
      'app/test/unit/**/*-test.{ts,tsx,js,jsx,mts,mjs}',
      'app/test/unit/**/*.test.{ts,tsx,js,jsx,mts,mjs}',
      'script/**/*-test.{ts,tsx,js,jsx,mts,mjs}',
      'script/**/*.test.{ts,tsx,js,jsx,mts,mjs}',
      'rust/tests/**/*-test.{ts,tsx,js,jsx,mts,mjs}',
      'rust/tests/**/*.test.{ts,tsx,js,jsx,mts,mjs}',
      'oxlint-rules/tests/**/*-test.{ts,tsx,js,jsx,mts,mjs}',
      'oxlint-rules/tests/**/*.test.{ts,tsx,js,jsx,mts,mjs}',
    ],
    maxWorkers: 2,
    testTimeout: 15_000,
    hookTimeout: 60_000,
    environment: 'jsdom',
    environmentOptions: { jsdom: { url: 'http://localhost/' } },
    setupFiles: ['./app/test/vitest-setup.ts'],
    // react-confetti imports `react/jsx-runtime` without an extension; Node's
    // ESM resolver rejects that for React 16 (no `exports` map), Vite does not.
    server: { deps: { inline: ['react-confetti'] } },
  },
})
