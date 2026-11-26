# Rust migration overview

This document is the current architectural summary. The detailed capability
matrix is in [`../docs/migration/status.md`](../docs/migration/status.md), the
ordered Linux worklist is in [`ROADMAP.md`](ROADMAP.md), and removal candidates
are tracked in [`../docs/migration/removals.md`](../docs/migration/removals.md).
Run `pnpm migration:status` for the current numeric baseline.

## Current architecture

- The user interface remains the React application in `app/`.
- Vite builds the renderer, syntax highlighter and webview IPC bootstrap into
  `out/vite-dist`.
- `rust/crates/desktop-app` is the active Tao/Wry desktop shell and serves the
  renderer through the Rust host bridge.
- `rust/crates/cli` (`desktop-rs`) is the headless CLI and can serve the same
  renderer over the authenticated loopback WebSocket bridge when built with
  `embedded-ui`.
- `rust/crates/core` is the definitive headless domain crate. The older shell
  domain crate is named `desktop-shell-core-legacy` while its useful services
  are merged or retired.
- `rust/crates/native` is a legacy host, not the target architecture.

Electron, Webpack and esbuild have been removed from the active build, package,
CI and root dependency graph. Release artifacts come from the Rust packaging
path (`pnpm package`), which is still a development package rather than a signed
installer.

## Current verification toolchain

- pnpm 11.17.0 is the JavaScript package manager; `pnpm-lock.yaml` is
  authoritative for the workspace. Cargo uses the root `Cargo.lock`.
- TypeScript 7 is the canonical `typescript` package and provides the fast
  `tsc` used by `pnpm check:types` and `pnpm compile:script`.
- Development-script entry points execute through `@oxc-node/core/register`;
  the TypeScript 6 compiler API compatibility package has been removed.
- oxlint is the JavaScript/TypeScript linter. Project rules live in
  `oxlint-rules/`; run `pnpm lint:oxlint`, `pnpm lint:security` and
  `pnpm test:oxlint`.
- Biome formats the repository; run `pnpm format:check` or `pnpm format`.
  Biome's linter is disabled in favor of oxlint.
- Vitest is the only TypeScript test runner. Older files may still import the
  `node:test` API surface, but `vitest.config.ts` aliases those imports to
  `app/test/helpers/node-test-compat.ts`; there is no separate `node:test` run
  or suite-routing file.
- Rust validation uses `cargo fmt`, `cargo clippy` and `cargo test` through the
  commands in [`ROADMAP.md`](ROADMAP.md) and CI.

## Measured migration baseline

Measured on 2026-11-25 with `pnpm migration:status`:

| Measure | Current | Done when |
| --- | --- | --- |
| Fallback checks (`transportKind`) in renderer | 2 in 2 files | 0 |
| Legacy Git runner calls (`await git(`) in `lib/git` | 8 in 6 files | 0 |
| Renderer files importing `dugite` | 0 | 0 |
| Renderer files importing `fs` | 0 | 0 (host operations only) |
| Renderer files importing `child_process` | 0 | 0 |
| Renderer files importing `os` / `crypto` / `keytar` | 0 / 0 / 0 | 0 |
| Unit test files on the Node runner | 0 of 426 | 0 |
| Rust code (crates, non-test) | 92,286 lines | — |

These numbers mean the test-runner migration is complete, but the renderer still
contains Node-backed Git and filesystem implementations. They are not Electron
runtime code; they are fallback implementations retained until each operation is
proven against the Rust backend. [`ROADMAP.md`](ROADMAP.md) is the authoritative
worklist for retiring them module by module.

## What has already been removed

- Electron main-process source, crash window, legacy launcher, Electron packager
  and installer dependencies, Electron-specific E2E harness, and Electron type
  declarations.
- Webpack and esbuild configuration/dependencies for the root renderer build.
- The root npm lockfile and the root ESLint/Prettier toolchain, including the
  old custom rule implementations and suite-routing file `vitest-suites.json`.
- Native addon dependencies whose responsibilities are represented by web shims
  or Rust code (`keytar`, `registry-js`, `desktop-notifications`,
  `fs-admin`). `desktop-trampoline` and `dugite` remain
  real dependencies until their callers are retired.
- Python smoke/integration paths that have Rust or Vitest replacements.

The renderer IPC vocabulary remains intentionally compatible with the typed
channels implemented by the Rust host. Compatibility shims in `app/src/web/` are
migration artifacts and must be removed individually only after their consumers
and tests use the typed Rust backend.

## Removal gates

No gate is satisfied by CLI passthrough alone, source counts, or the existence
of a replacement file. Legacy removal requires runtime evidence of replacement.

- [ ] Inventory and replace every user-visible legacy workflow, including
      GitHub, credentials, repository management, settings, integration and
      recovery paths.
- [ ] Move reusable upstream web components/models/styles/assets into a
      standalone frontend; replace their Node and synchronous IPC dependencies.
- [ ] Complete typed Git workflows with normal and failure/recovery coverage.
- [ ] Complete GitHub operations through the Rust/GitHub CLI path, including
      PRs, issues, checks, enterprise/authentication, and actionable failures.
- [ ] Finish repository creation/clone, multi-repository persistence, branch
      upstream state, history/diffs, selection/partial staging, binary/image
      diffs, and conflict UX.
- [ ] Validate the Tao/Wry host on supported desktop targets; keep Termux and
      headless builds free from desktop-only dependencies.
- [ ] Validate browser user journeys, reconnects, large repositories, Unicode
      and byte-exact filenames, cancellation, security, and state consistency.
- [ ] Replace release packaging, signing, update distribution and license
      inventory with production-quality Rust/Vite paths.
- [ ] Remove legacy source, packages, native modules, assets/scripts and stale
      documentation only after the replacement evidence exists.
- [ ] Build and exercise the resulting repository without legacy directories
      present.

## Routine validation commands

```sh
pnpm install --frozen-lockfile --ignore-scripts
pnpm lint
pnpm check:types
pnpm compile:script
pnpm migration:check
pnpm vite:build
pnpm test
pnpm test:script
pnpm test:oxlint
cargo fmt --all -- --check
cargo clippy --workspace --exclude desktop-app --all-targets --locked -- -D warnings
cargo test --workspace --exclude desktop-app --locked
```

For a desktop-shell build use `pnpm build:dev`; for the development package use
`pnpm package`. GUI validation currently has Linux evidence under Xvfb; native
macOS and Windows behavior remains a migration gate.
