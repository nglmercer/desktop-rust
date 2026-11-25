# Setting up development dependencies on macOS

You need these tools:

- Node.js 22.12 or newer (the recommended version is in `.nvmrc`/`.node-version`)
- pnpm 11.17.0
- Rust stable with `rustfmt` and `clippy`
- Python 3
- Xcode Command Line Tools

## Node.js and pnpm

Check the versions:

```shellsession
$ node -v
$ pnpm -v
```

If Node.js is missing, install it from the [Node.js website](https://nodejs.org/)
or use `nvm`/`asdf` with the version file in this repository. Node includes npm,
which can be used once to install the pinned pnpm version:

```shellsession
$ npm install --global pnpm@11.17.0
```

Use pnpm, not npm, for workspace dependency installation. See
[working with packages](./working-with-packages.md).

## Rust

Install Rust through [rustup](https://rustup.rs/) and verify:

```shellsession
$ cargo --version
$ rustc --version
```

## Python 3

macOS normally includes a usable Python 3. Verify with:

```shellsession
$ python3 --version
```

Use [pyenv](https://github.com/pyenv/pyenv) if you need multiple Python versions.

## Xcode Command Line Tools

The Rust linker and platform tooling need the command-line developer tools:

```shellsession
$ xcode-select --install
```

macOS uses WKWebView through Wry; no separate WebView download is required.

## Back to setup

Return to the [setup page](./setup.md) to install dependencies and build the app.
