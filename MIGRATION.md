# Electron → `tao` + `wry` migration (Rust-only backend)

This branch replaces the Electron main process and the Node-enabled renderer
with a Rust shell. The window is created by
[`tao`](https://crates.io/crates/tao), the web content is hosted by
[`wry`](https://crates.io/crates/wry), and **Tauri is not used** — there is no
Tauri runtime, no `tauri.conf.json`, no `#[tauri::command]` and no Node/Electron
process. The user interface is the React application in `app/`, built by Vite
(`pnpm vite:build`) into `out/vite-dist`. Host services are Rust, but some
application logic is still TypeScript in `app/`.

## Status

| Area | Implemented | Connected | Verified |
|------|-------------|-----------|----------|
| Window, webview, `desktop://` protocol, bundle serving | yes | yes (Linux, Xvfb) | startup smoke: `rust/crates/desktop-app/tests/app_smoke.rs` |
| IPC bridge: admission lanes, bounded queues, `EBUSY` refusals | yes | yes | `desktop-app` and `desktop-core` unit tests |
| Typed Wry repository backend | snapshot and porcelain-v2 status reads | normal Wry status refresh | Rust real-Git status and repository-isolation tests; frontend routing tests; cancellation and mutations pending |
| Process spawn with stdin/stdout flow control | yes | yes (Copilot runtime path) | Rust tests; renderer adapter tests (`app/test/unit/web-child-process-test.ts`) |
| git execution with `maxBuffer`, ceiling and cancellation | yes | yes | Rust tests (`io.rs`) and adapter (`dugite.ts`) reviewed, not unit-tested |
| File range reads and native hashes | yes | yes (`fs.ts`, `get-file-hash.ts`) | Rust tests; renderer tests (`web-fs-range-test.ts`) |
| Renderer-side UI flows (clicks, menus, Git operations) | React UI | yes | not automated: no GUI driver in this tree |
| Browser / WebSocket transport for Termux | typed transport | authenticated bridge; no browser UI | real-bridge authentication, events, cancellation and reconnect test; browser entry still returns 503 |
| Copilot parity, multi-account Git auth, installers, updater | partial | partial | pending |

The headless core uses a native Copilot turn transport. It uses the runtime's
framed RPC protocol and falls back to the older send/event protocol only after
a method-not-found response. Sessions stay in memory until explicitly closed
or their repository handle is dropped. Live authenticated parity and restart
recovery have not been verified. The renderer continues
to use its account-scoped SDK path for commit messages: the headless Copilot
service does not yet preserve the selected account, provider, model and rules.
Runtime discovery maps Rust targets to the SDK's platform names and selects
the pinned package version when searching npm or pnpm installations.


## Why this shape

The original renderer ran with `nodeIntegration: true` and
`contextIsolation: false`. It imported `fs`, `path`, `dugite`, `keytar`,
`api.ts` and `electron` directly, so the "main process" boundary was largely
notional. A `wry` webview has no Node runtime, so those capabilities had to
become explicit commands owned by Rust. The migration therefore splits the
application in two:

| Crate | Responsibility | GUI dependencies |
|-------|----------------|------------------|
| `rust/crates/desktop-core` | models, git engine, storage, GitHub API, platform helpers, app state, IPC dispatch | **none** |
| `rust/crates/desktop-app` | tao window/event loop, wry webview, custom protocol, native menu, dialogs, clipboard | tao, wry, muda, rfd, arboard |

`desktop-core` talks to the shell exclusively through the `Host` trait
(`emit` and `window`). That keeps the domain layer unit-testable without a
window or a display server — `cargo test -p desktop-core` runs headless.

## Layout

```
rust/                                 Rust crates (root Cargo workspace)
├── crates/desktop-core/src/          no GUI dependencies
│   ├── io.rs                         desktop.io.* host services: git, fs, HTTP, keychain
│   ├── io/limits.rs                  finite budgets per resource profile
│   ├── io/executor.rs                fixed workers, bounded non-blocking admission
│   ├── io/process.rs                 child processes with stdin/stdout flow control
│   ├── models/, git/, storage/, api/, platform/, ipc/, state/, app.rs
└── crates/desktop-app/
    ├── src/main.rs                   tao event loop + wry webview
    ├── src/bridge.rs                 desktop:// protocol and IPC admission lanes
    ├── src/host.rs                   Host implementation, outbox and Flush batching
    ├── src/actions.rs, menu.rs       WindowAction on the UI thread, muda menu
    ├── build.rs                      rebuilds the renderer when its inputs change
    └── web/ipc-shim.js               the only file of web/ still in use (injected)
app/                                  the React renderer (active), built by Vite
out/vite-dist/                        renderer bundle: embedded in release,
                                      served from disk in debug
```

The root `Cargo.toml` owns all crates. Default members are the headless CLI,
core and bridge; the desktop shell and native host are built explicitly.

## The IPC bridge

| Electron | Replacement |
|----------|-------------|
| `ipcRenderer.invoke` | correlated `window.ipc.postMessage` → Rust worker → `evaluate_script("__desktop.resolve(…)")` |
| `ipcRenderer.send` | `window.ipc.postMessage` → Rust worker → `DesktopApp::send` |
| `ipcWebContents.send` | `Host::emit` → outbox → `evaluate_script("__desktop.emit(…)")` |
| `ipcRenderer.sendSync` | promise-returning `sendSync` (wry has no synchronous IPC; the three `will-quit*` channels must be awaited) |
| `file://index.html` + Node integration | `desktop://localhost/index.html` custom protocol, no Node |
| native menu (`Menu.buildFromTemplate`) | `muda`, ids equal to the original `MenuEvent` strings |
| `dialog` / `clipboard` / `shell` | `rfd` / `arboard` / `platform` module, all on Rust |

An initialization script (`web/ipc-shim.js`) installs an
`ipcRenderer`-compatible object plus `window.desktop`. A minimal
`require('electron')` shim is provided for a browser build of the legacy
renderer; any other Node module throws with a message pointing at the
`desktop.*` command that should be used instead.

`invoke` deliberately does not use `fetch`: custom schemes have
platform-specific origins on WebView2/WKWebView, which makes CORS fragile.
A `desktop://localhost/__invoke` POST endpoint is nevertheless provided for
headless smoke tests and non-webview clients.

### Channels

* 28 duplex channels (`ipcMain.handle`)
* 31 inbound simplex channels (`ipcMain.on`)
* 20 push-only main-process → renderer channels
* 79 total legacy channels, all named in `ipc/channels.rs` and covered by a
  unit test
* plus the `desktop.*` namespace for capabilities that used to be direct
  renderer imports: repository management, status/log/refs/diff, stage,
  commit, fetch/pull/push/clone, branch/stash/merge/rebase/cherry-pick/revert,
  tags/remotes/config, accounts and GitHub API calls, preferences and dialogs.

### State ownership

Electron's `AppStore` lived in the renderer. Here `AppState` is owned by
`DesktopApp` behind a mutex. Mutations persist atomically to JSON and push a
sanitized `state-changed` snapshot. Account tokens and `endpointTokens` are
**never** serialized into a snapshot (unit-tested). Secrets live in
DPAPI-protected credential files on Windows via `SecureStore`.

Events emitted before the renderer subscribes are buffered in the host outbox
and replayed in order after `renderer-ready`, so no initial state is lost.

## Build and run

```bash
# 1. renderer dependencies (pnpm is pinned by packageManager in package.json)
pnpm install --frozen-lockfile --ignore-scripts
# 2. renderer bundle; needs the submodules: git submodule update --init --recursive
pnpm vite:build
# 3. desktop shell
cargo build -p desktop-app --locked
./rust/target/debug/github-desktop-rs

# checks
cargo test --workspace --locked
xvfb-run -a cargo test -p desktop-app --test app_smoke --locked -- --ignored
```

`cargo build` also rebuilds the bundle when an input is newer than it, by
running `pnpm vite:build`. A failed Vite build stops Cargo with its cause.

Limits are chosen with `DESKTOP_RESOURCE_PROFILE=default|low-memory`. Parallel
compilation is `CARGO_BUILD_JOBS`; `.cargo/config.toml` defaults to one job.
Source maps are off by default; `DESKTOP_VITE_SOURCEMAP=1 pnpm vite:build`
writes them to `out/vite-sourcemaps`, outside the embedded bundle.

CLI options: `--version`, `--help`, `--self-test`, `--cli-open=<path>`,
`--cli-clone=<url>`. Any `x-github-client://` / `github-mac://` argument is
forwarded to the web layer as a `url-action` event. Set `RUST_LOG=debug` for
verbose tracing.

Requirements: Rust 1.85+, a system `git`, and the platform webview runtime
(WebView2 on Windows, WKWebView on macOS, WebKitGTK on Linux). On Linux,
`pkg-config` and the WebKitGTK development packages are required.

## Scope and known limitations

The shell starts with the existing React frontend, but complete end-to-end
workflow parity has not been established. Most renderer workflows still use
compatibility shims instead of typed Rust operations. Normal Wry status refresh
uses byte-preserving Rust porcelain-v2 records under the existing read lease;
strict-error calls and failed or oversized Rust reads still use the legacy Git
path. The typed Wry backend does not yet support cancellation, mutations,
account selection or Copilot sessions.
The browser transport can authenticate, deliver events, cancel requests and
reconnect without replaying in-flight work. The React UI still depends on
shell-only IPC, and the bridge does not serve a browser frontend yet.
The following areas also need implementation or verification:

* interactive rebase, conflict resolution UI, LFS, submodule management;
* the full auto-updater (Squirrel / Darwin update flow) — `check-for-updates`
  is a no-op stub;
* OAuth device flow (sign-in currently accepts a personal access token);
* the complete web-rendered equivalent of every preferences pane and the
  tutorial repository flow;
* crash-window / error-reporting UI.

The `desktop.*` command surface is intentionally additive, so those features
can be ported incrementally without changing the transport.

## Security notes

* No `nodeIntegration`, no `file://` origin, no remote code loading.
* A navigation handler only allows `desktop://` (and its WebView2 origin
  equivalent) plus `about:` URLs.
* The custom protocol rejects path traversal (`. .`, `\`, `:`).
* No Content-Security-Policy is set yet. Adding one to the renderer is pending.
* Credential ownership is still being migrated; the current Copilot renderer
  path receives account credentials and must be replaced with an equivalent
  account-scoped Rust implementation before claiming a Rust-only backend.
* JSON state is written via a temp file + rename for atomicity.
