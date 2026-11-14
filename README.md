# Desktop Rust migration

This checkout is migrating GitHub Desktop to Rust. The desktop UI that runs
today is the React app in `app/`, built with Vite and hosted by a tao/wry shell
in `rust/crates/desktop-app`. Host services (git, files, HTTP, processes) are
Rust in `rust/crates/desktop-core`; parts of the application logic are still
TypeScript. The browser/WebSocket renderer is available as an experimental
opt-in build; the default `desktop-rs serve` exposes the API but returns 503
for the page until the frontend is included.

Build and start the desktop shell:

```sh
git submodule update --init --recursive           # bundled renderer resources
pnpm install --frozen-lockfile --ignore-scripts   # renderer dependencies
pnpm vite:build                                   # renderer bundle -> out/vite-dist
cargo build -p desktop-app --locked
./rust/target/debug/github-desktop-rs
```

`pnpm build:dev` builds the renderer, Rust desktop and embedded-browser CLI
without starting them. It rejects a Cargo cross-target because renderer platform
flags follow Node's host. Node and the Rust toolchain must also use the same
host OS and architecture; that combination is not checked automatically. Vite
updates `out/vite-dist` before Cargo rejects an unsupported target.

`pnpm build:prod` builds optimized Rust desktop and browser-enabled CLI
binaries; `pnpm package` stages a relocatable Rust package under `dist/`. These
are migration artifacts, not release installers: distribution, signing,
updates, complete license inventories and feature parity remain unfinished.
After packaging, `pnpm start:prod` starts that staged desktop. Copilot uses its
adjacent runtime when it is installed. Set `DESKTOP_RUST_DIST_DIR` to an
absolute directory to stage and launch a package elsewhere. Close other Rust
desktop instances first: state writes are not yet coordinated across processes.
On Linux, the Rust shell registers the callback scheme selected by the bundled
OAuth credentials. Existing handlers are left unchanged. To select this
installation for production credentials, run
`xdg-mime default github-desktop-rs-production.desktop x-scheme-handler/x-github-desktop-auth`.
For development credentials, select `github-desktop-rs-dev-v2.desktop` for
`x-scheme-handler/x-github-desktop-dev-auth` instead. The older Rust desktop
entry is retained because it does not identify its profile.
The OAuth application must be configured for that callback; real production
sign-in has not been verified. Custom builds must supply both
`DESKTOP_OAUTH_CLIENT_ID` and `DESKTOP_OAUTH_CLIENT_SECRET`, or neither.
The generated entry is tied to its executable
and data directory. If either changes, the app refuses to overwrite the old
entry: close the old instance and remove its entry before registering again.
Registration on macOS and Windows remains pending. While
`pnpm start:prod` is running, another invocation for the same staged package is
blocked; it cannot forward URLs or launch actions to the open instance through
that command.

Production builds and packages use a private, temporary Vite bundle, so the
development watcher can continue using `out/vite-dist`. Direct Cargo builds
with `desktop-app/packaged-ui` require an absolute `DESKTOP_VITE_DIST` path
to a complete, freshly built bundle outside the development directory. Package
builds use a separate `desktop-packaged` subdirectory under Cargo's target
directory and cooperative locks on that target and the package destination.
External Cargo commands do not respect these locks. If a package operation or
staged host is killed unexpectedly, a lock can remain: inspect its recorded
owner and verify that no related host or build process is running before
manually removing it. A force-killed packaging parent may leave its temporary
Vite directory and child build process behind.
The existing Electron production pipeline is available through
`pnpm build:electron:prod` and `pnpm package:electron` until it is replaced.

For development, `pnpm start` builds and starts the Rust shell and watches the
Vite renderer, highlighter and IPC bootstrap. After a successful rebuild and
after Git operations finish, use View > Reload (`Ctrl`/`Cmd` + `Alt` + `R`) in
the app to apply the changes.
Close the window or press Ctrl-C in the terminal to stop the shell and watchers.
The legacy Electron development launcher remains available as `pnpm start:electron`
during migration. `pnpm start:rust` is an alias for the default launcher.
The Rust launcher and its shutdown path have been exercised
on Linux; macOS and Windows still need native validation.
If a copied static resource is removed, stop the launcher, run `pnpm vite:build`,
and restart it so the old generated file is cleared.
Restart the launcher after changing Vite configuration, dependencies, or the
license-generation code, which are loaded when the watcher starts.

To try the same renderer over the loopback WebSocket bridge, build the web
bundle above, then run:

```sh
cargo build -p desktop-cli --features embedded-ui --locked
./rust/target/debug/desktop-rs serve --empty
```

The printed URL contains a session secret. The browser workflow is still
undergoing parity and recovery testing; see the migration ledger before relying
on it as a replacement for the existing desktop application.

`pnpm` is pinned by `packageManager` in `package.json`. The root Cargo workspace
contains the headless CLI, bridge, native host, and desktop shell. Its default
members build without GUI dependencies using `cargo build --locked`.

Unit tests are moving from Node's test runner to Vitest. `npm test -- <path>`
routes migrated suites through Vitest; `vitest-suites.json` lists those suites.
Run them together with `node node_modules/vitest/vitest.mjs run --config vitest.config.ts`.
Biome checks the test-runner tooling and project-specific rules via
`npm run lint:biome` and `npm run lint:security`. Prettier and ESLint still
cover other files while their migration is in progress.

See [Rust development and commands](rust/README.md) and the
[full migration ledger](rust/MIGRATION.md). The Electron-era `postinstall`
script in `package.json` is not used by the Rust shell; `--ignore-scripts`
skips it.
This is an in-progress fork, not an official GitHub Desktop release.

---

# [GitHub Desktop](https://desktop.github.com)

[GitHub Desktop](https://desktop.github.com/) is an open-source [Electron](https://www.electronjs.org/)-based
GitHub app. It is written in [TypeScript](https://www.typescriptlang.org) and
uses [React](https://reactjs.org/).

<picture>
  <source
    srcset="https://user-images.githubusercontent.com/634063/202742848-63fa1488-6254-49b5-af7c-96a6b50ea8af.png"
    media="(prefers-color-scheme: dark)"
  />
  <img
    width="1072"
    src="https://user-images.githubusercontent.com/634063/202742985-bb3b3b94-8aca-404a-8d8a-fd6a6f030672.png"
    alt="A screenshot of the GitHub Desktop application showing changes being viewed and committed with two attributed co-authors"
  />
</picture>

## Where can I get it?

Download the official installer for your operating system:

 - [macOS](https://central.github.com/deployments/desktop/desktop/latest/darwin)
 - [macOS (Apple silicon)](https://central.github.com/deployments/desktop/desktop/latest/darwin-arm64)
 - [Windows](https://central.github.com/deployments/desktop/desktop/latest/win32)
 - [Windows machine-wide install](https://central.github.com/deployments/desktop/desktop/latest/win32?format=msi)

Linux is not officially supported; however, you can find installers created for Linux from a fork of GitHub Desktop in the [Community Releases](https://github.com/desktop/desktop#community-releases) section.

### Beta Channel

Want to test out new features and get fixes before everyone else? Install the
beta channel to get access to early builds of Desktop:

 - [macOS](https://central.github.com/deployments/desktop/desktop/latest/darwin?env=beta)
 - [macOS (Apple silicon)](https://central.github.com/deployments/desktop/desktop/latest/darwin-arm64?env=beta)
 - [Windows](https://central.github.com/deployments/desktop/desktop/latest/win32?env=beta)
 - [Windows (ARM64)](https://central.github.com/deployments/desktop/desktop/latest/win32-arm64?env=beta)

The release notes for the latest beta versions are available [here](https://desktop.github.com/release-notes/?env=beta).

### Past Releases
You can find past releases at https://desktop.githubusercontent.com. After installation of a past version, the auto update functionality will attempt to download the latest version. 

### Community Releases

There are several community-supported package managers that can be used to
install GitHub Desktop:
 - Windows users can install using [winget](https://docs.microsoft.com/en-us/windows/package-manager/winget/) `c:\> winget install github-desktop` or [Chocolatey](https://chocolatey.org/) `c:\> choco install github-desktop`
 - macOS users can install using [Homebrew](https://brew.sh/) package manager:
      `$ brew install --cask github`

Installers for various Linux distributions can be found on the
[`shiftkey/desktop`](https://github.com/shiftkey/desktop) fork.

## Is GitHub Desktop right for me? What are the primary areas of focus?

[This document](https://github.com/desktop/desktop/blob/development/docs/process/what-is-desktop.md) describes the focus of GitHub Desktop and who the product is most useful for.

## I have a problem with GitHub Desktop

Note: The [GitHub Desktop Code of Conduct](https://github.com/desktop/desktop/blob/development/CODE_OF_CONDUCT.md) applies in all interactions relating to the GitHub Desktop project.

First, please search the [open issues](https://github.com/desktop/desktop/issues?q=is%3Aopen)
and [closed issues](https://github.com/desktop/desktop/issues?q=is%3Aclosed)
to see if your issue hasn't already been reported (it may also be fixed).

There is also a list of [known issues](https://github.com/desktop/desktop/blob/development/docs/known-issues.md)
that are being tracked against Desktop, and some of these issues have workarounds.

If you can't find an issue that matches what you're seeing, open a [new issue](https://github.com/desktop/desktop/issues/new/choose),
choose the right template and provide us with enough information to investigate
further.

## The issue I reported isn't fixed yet. What can I do?

If nobody has responded to your issue in a few days, you're welcome to respond to it with a friendly ping in the issue. Please do not respond more than a second time if nobody has responded. The GitHub Desktop maintainers are constrained in time and resources, and diagnosing individual configurations can be difficult and time consuming. While we'll try to at least get you pointed in the right direction, we can't guarantee we'll be able to dig too deeply into any one person's issue.

## How can I contribute to GitHub Desktop?

The [CONTRIBUTING.md](./.github/CONTRIBUTING.md) document will help you get setup and
familiar with the source. The [documentation](docs/) folder also contains more
resources relevant to the project.

If you're looking for something to work on, check out the [help wanted](https://github.com/desktop/desktop/issues?q=is%3Aissue+is%3Aopen+label%3A%22help%20wanted%22) label.

## Building Desktop

To setup your development environment for building Desktop, check out: [`setup.md`](./docs/contributing/setup.md).

## More Resources

See [desktop.github.com](https://desktop.github.com) for more product-oriented
information about GitHub Desktop.

See our [getting started documentation](https://docs.github.com/en/desktop/overview/getting-started-with-github-desktop) for more information on how to set up, authenticate, and configure GitHub Desktop.

## License

**[MIT](LICENSE)**

The MIT license grant is not for GitHub's trademarks, which include the logo
designs. GitHub reserves all trademark and copyright rights in and to all
GitHub trademarks. GitHub's logos include, for instance, the stylized
Invertocat designs that include "logo" in the file title in the following
folder: [logos](app/static/logos).

GitHub® and its stylized versions and the Invertocat mark are GitHub's
Trademarks or registered Trademarks. When using GitHub's logos, be sure to
follow the GitHub [logo guidelines](https://github.com/logos).
