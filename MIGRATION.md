# Rust migration documentation

The maintained migration documentation now lives in:

- [`rust/MIGRATION.md`](rust/MIGRATION.md) — architecture, validation rules, and the routine
  verification gate.
- [`rust/ROADMAP.md`](rust/ROADMAP.md) — remaining migration worklist.
- [`docs/migration/status.md`](docs/migration/status.md) — current capability status.
- [`docs/migration/removals.md`](docs/migration/removals.md) — retained compatibility pieces and removal
  gates.

This root file is intentionally only a pointer. Keeping a second migration
ledger here caused the historical document to contradict the active toolchain
and migration state.
