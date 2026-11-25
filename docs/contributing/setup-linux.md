# Setting up development dependencies on Linux

You need these tools:

- Node.js 22.12 or newer (the recommended version is in `.nvmrc`/`.node-version`)
- pnpm 11.17.0
- Rust stable with `rustfmt` and `clippy`
- Python 3
- a C linker and standard build tools
- WebKitGTK/GTK/D-Bus development packages for the desktop shell

## Node.js and pnpm

Install Node.js using your distribution or the instructions from the
[Node.js website](https://nodejs.org/). Then install the pinned package manager:

```shellsession
$ npm install --global pnpm@11.17.0
```

Verify:

```shellsession
$ node -v
$ pnpm -v
```

Use pnpm, not npm, for workspace dependency installation. See
[working with packages](./working-with-packages.md).

## Rust and build tools

Install Rust through [rustup](https://rustup.rs/) and verify:

```shellsession
$ cargo --version
$ rustc --version
```

Install the normal C toolchain and package-config support using your
distribution, for example `build-essential`/`pkg-config` on Debian or Ubuntu or
`gcc`/`pkgconf` on Fedora.

## Python 3

Install Python 3 with your distribution's package manager and verify:

```shellsession
$ python3 --version
```

## Webview dependencies

The Rust desktop host renders with WebKitGTK. Debian/Ubuntu example:

```shellsession
$ sudo apt install libsecret-1-dev libwebkit2gtk-4.1-dev libgtk-3-dev libdbus-1-dev libxdo-dev
```

Fedora example:

```shellsession
$ sudo dnf install -y libsecret-devel webkit2gtk4.1-devel gtk3-devel dbus-devel libxdo-devel
```

The headless CLI and most Rust tests can build without these GUI libraries when
`desktop-app` is excluded. GUI smoke tests additionally use Xvfb and related X11
packages in CI.

## Back to setup

Return to the [setup page](./setup.md) to install dependencies and build the app.
