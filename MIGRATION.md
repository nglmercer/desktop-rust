# Rust migration documentation

The maintained migration documentation now lives in:

- [`rust/MIGRATION.md`](rust/MIGRATION.md) — current architecture, evidence and
  verification commands.
- [`rust/ROADMAP.md`](rust/ROADMAP.md) — ordered module-by-module migration
  worklist.
- [`docs/migration/status.md`](docs/migration/status.md) — measured fallback
  inventory.
- [`docs/migration/removals.md`](docs/migration/removals.md) — retired pieces,
  retained shims and removal gates.

This root file is intentionally only a pointer. Keeping a second migration
ledger here caused the historical document to contradict the active toolchain
and migration state.
