# Troubleshooting

Here's a non-exhaustive list of environmental issues that you may encounter
while working on GitHub Desktop.

## Dependencies are out of sync

Use the pinned package manager from the repository root:

```sh
git submodule update --init --recursive
pnpm install --frozen-lockfile --ignore-scripts
```

Do not run `npm install` in the root workspace. The root npm lockfile has been
retired along with the nested `app/package-lock.json`; `pnpm-lock.yaml` is
authoritative.

If the install appears correct but scripts report missing executables, remove
only the generated dependency directories and reinstall:

```sh
rm -rf node_modules app/node_modules
pnpm install --frozen-lockfile --ignore-scripts
```

## Linux webview build failures

The Rust shell uses Tao/Wry. On Debian/Ubuntu-like systems install the WebKitGTK
and D-Bus development packages before building `desktop-app`:

```sh
sudo apt-get install libwebkit2gtk-4.1-dev libgtk-3-dev libdbus-1-dev
```

CI also installs Xvfb and related X11/GL packages for the GUI smoke test. The
headless CLI/workspace build deliberately excludes `desktop-app` and does not
need those GUI libraries.

## TypeScript version confusion

`node_modules/.bin/tsc` is TypeScript 7. TypeScript 6 is still present as the
`typescript` package for compiler-API compatibility. If a script fails because
it imports the compiler API, do not replace it with the TypeScript 7 CLI: keep
the TypeScript 6 dependency aligned or migrate the script away from the compiler
API deliberately.

## Renderer changes do not appear

Vite performs bundling but not typechecking. Run:

```sh
pnpm check:types
pnpm vite:build
```

When using the watcher started by `pnpm start`, reload the webview with
<kbd>Ctrl/Command+Alt+R</kbd> after a successful renderer rebuild. Restart the
launcher after changing Vite configuration, dependencies, or license-generation
code.

## Tests need the Rust backend

On Linux, `pnpm test` builds `desktop-cli` before Vitest and uses the real Rust
backend by default. If Cargo or the backend build is broken, fix that first;
setting `DESKTOP_TEST_RUST_BACKEND=0` is only a temporary diagnostic because
tests for already-migrated operations no longer have Node fallbacks.
