# Building and Packaging Desktop

This document outlines how Desktop is built and packaged.

## Vite

GitHub Desktop uses Vite to transpile and bundle its application resources.
The configuration files are found at the repository root:

 - `vite.config.ts` - the Rust-hosted renderer
 - `vite.highlighter.config.ts` - `highlighter.js`, the syntax-highlighting
   worker

Vite also handles these steps:

 - placeholders in source for platform-specific values are replaced, using
   `app/app-info.ts` and the values of the platform the build is being performed
   on
 - SCSS stylesheets under `app/styles/` are transpiled to CSS and emitted
 - source maps are generated to help with correlating runtime errors to the
   corresponding TypeScript source

Vite does not type-check; run `npm run check:types` separately.

The output is stored in `out/`, which is ignored in version control.

## `app/package.json`

The `version` attribute in `app/package.json` is the canonical reference for
the version number you see in the **About** view of GitHub Desktop.

## `changelog.json`

This file is used to track user-facing changes associated with each Desktop
release. This can include new features, bug fixes, improvements or even removed
features.

## Rust packaging

`pnpm package` (`script/package-rust.ts`) is the only packaging path. It builds
the Vite renderer, the browser-enabled CLI with the embedded renderer and the
Wry desktop shell, then stages them with the Copilot runtime, license inventory
(`meta/rust-dependencies.json`) and bundled resources into a package under
`dist/`. This is a development package, not a signed installer; see
`rust/MIGRATION.md` for what packaging does and does not yet cover.
