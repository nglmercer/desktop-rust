# Rust host development

This directory contains the Rust replacement for GitHub Desktop's host,
domain logic, CLI, and transports. The React renderer in `app/` remains the
user interface; Vite builds it and `rust/crates/desktop-app` hosts it with
Tao/Wry.

Migration status and remaining work are tracked in:

- [`MIGRATION.md`](MIGRATION.md) — architecture and validation rules
- [`ROADMAP.md`](ROADMAP.md) — ordered migration worklist
- [`TODO.md`](TODO.md) — platform and release gaps
- [`../docs/migration/status.md`](../docs/migration/status.md) — current capability status
- [`../docs/migration/removals.md`](../docs/migration/removals.md) — removal gates

## Layout

- `crates/core` — headless repository, Git, GitHub, credential, and update domain logic.
- `crates/bridge` — authenticated loopback WebSocket/HTTP bridge.
- `crates/cli` — `desktop-rs` command-line entry point and test server.
- `crates/desktop-app` — active Tao/Wry desktop shell.
- `crates/desktop-shell-core-legacy` — legacy shell services being merged or retired.
- `crates/native` — legacy independent host, not the target architecture.
- `tests` — Rust and TypeScript integration helpers.

## Routine commands

Run JavaScript commands from the repository root:

```sh
pnpm install --frozen-lockfile --ignore-scripts
pnpm vite:build
pnpm check:types
pnpm lint
pnpm migration:check
pnpm test
```

Run Rust commands from the repository root unless a crate document says otherwise:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --exclude desktop-app --all-targets --locked -- -D warnings
cargo test --workspace --exclude desktop-app --locked
cargo clippy -p desktop-app --all-targets --locked -- -D warnings
cargo test -p desktop-app --locked
```

The desktop-shell tests need the platform GUI dependencies. On Linux the
renderer smoke test also needs a display, normally through Xvfb.

## Development shell

```sh
pnpm build:dev
pnpm start
```

`pnpm package` creates the current development package. It is not a signed or
production-ready installer.

## Headless CLI

```sh
cargo build -p desktop-cli --locked
./rust/target/debug/desktop-rs --help
./rust/target/debug/desktop-rs serve --port 0
```

`serve` starts an authenticated loopback WebSocket API. Treat the printed URL
as a credential. The optional `embedded-ui` feature serves the Vite renderer
through the same bridge; build the renderer first with `pnpm vite:build`.

## Validation principle

A replacement is not evidence of migration by itself. Keep runtime test
evidence with every removal: focused Vitest/Rust tests, typecheck/lint/format,
migration metrics, and the relevant shell or package smoke test. The full
validation gate is listed in [`MIGRATION.md`](MIGRATION.md).
