# Rust migration overview

This is the concise architectural reference for the migration. Use
[`ROADMAP.md`](ROADMAP.md) for remaining work,
[`../docs/migration/status.md`](../docs/migration/status.md) for capability
status, and [`../docs/migration/removals.md`](../docs/migration/removals.md)
for removal gates. Run `pnpm migration:status` for live metrics rather than
copying stale numbers into pull requests.

## Architecture

- `app/` contains the React renderer, built by Vite into `out/vite-dist`.
- `rust/crates/desktop-app` is the active Tao/Wry desktop shell.
- `rust/crates/core` is the intended headless domain crate.
- `rust/crates/bridge` exposes authenticated loopback WebSocket/HTTP access.
- `rust/crates/cli` provides `desktop-rs` and the test backend.
- `rust/crates/desktop-shell-core-legacy` and `rust/crates/native` are legacy
  migration artifacts, not the target architecture.

Electron, Webpack, esbuild, the root npm lockfile, and the old ESLint/Prettier
toolchain are out of the active build. The Rust package path exists but still
produces a development package, not a signed production installer.

## Verification toolchain

- pnpm with `pnpm-lock.yaml`; install using `--frozen-lockfile --ignore-scripts`.
- TypeScript 7 through `pnpm check:types` and `pnpm compile:script`.
- oxlint through `pnpm lint`; Biome formatting through `pnpm format:check`.
- Vitest as the only TypeScript test runner. `node:test` imports are aliases
  to the Vitest compatibility layer, not a separate runner.
- Cargo fmt, clippy, and tests for Rust crates.

## Removal rule

Do not remove legacy behavior merely because replacement source exists. A
removal needs runtime evidence that the replacement preserves the workflow,
including failure, cancellation, recovery, progress, and security behavior
where applicable. Compatibility shims in `app/src/web/` are removed one at a
time after all production consumers and shim-dependent tests are gone.

## Routine validation gate

```sh
pnpm install --frozen-lockfile --ignore-scripts
pnpm lint
pnpm check:types
pnpm compile:script
pnpm migration:check
pnpm format:check
pnpm vite:build
pnpm test
pnpm test:script
pnpm test:oxlint
cargo fmt --all -- --check
cargo clippy --workspace --exclude desktop-app --all-targets --locked -- -D warnings
cargo test --workspace --exclude desktop-app --locked
```

For shell changes also run:

```sh
cargo clippy -p desktop-app --all-targets --locked -- -D warnings
cargo test -p desktop-app --locked
```

For release validation use `pnpm build:prod` and `pnpm package`, then relocate
and run the package outside the checkout. Linux/Xvfb is the current primary
GUI evidence; macOS and Windows remain release gates.
