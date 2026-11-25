# Setting up development dependencies on Windows

You need these tools:

- Node.js 22.12 or newer (the recommended version is in `.nvmrc`/`.node-version`)
- pnpm 11.17.0
- Rust stable with the MSVC toolchain, `rustfmt` and `clippy`
- Python 3
- Microsoft C++ Build Tools
- WebView2 Runtime (normally preinstalled on supported Windows versions)

## Node.js and pnpm

Check the versions:

```shellsession
$ node -v
$ pnpm -v
```

Install Node.js from the [Node.js website](https://nodejs.org/) or use
`nvm-windows`. Allow the installer to add Node to `PATH`. Then install the pinned
package manager once:

```shellsession
$ npm install --global pnpm@11.17.0
```

Use pnpm, not npm, for workspace dependency installation. See
[working with packages](./working-with-packages.md).

## Rust

Install Rust through [rustup](https://rustup.rs/). The default Windows host
triple should use MSVC, for example `x86_64-pc-windows-msvc`. Verify:

```shellsession
$ cargo --version
$ rustc --version
```

## Python 3

Install Python 3 from the [Python website](https://www.python.org/downloads/windows/)
or the Microsoft Store. Ensure `python.exe` is on `PATH` and verify:

```shellsession
$ python --version
```

## Microsoft C++ Build Tools

Install the **Desktop development with C++** workload from Visual Studio Build
Tools. Rust's MSVC linker and some build scripts require the linker, Windows SDK
and MSVC runtime supplied by that workload. No `node-gyp` or npm MSVS configuration
is needed for the current Rust shell.

## Troubleshooting

If a development profile appears stuck, close Desktop and remove only the
development profile directory `C:\Users\[Your_User]\AppData\Roaming\GitHub Desktop-dev`.
Do not remove production data.

## Back to setup

Return to the [setup page](./setup.md) to install dependencies and build the app.
