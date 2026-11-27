# Migration status

This file summarizes current capability status. It is not a session log; use
`pnpm migration:status` for live numeric metrics and `rust/ROADMAP.md` for the
remaining worklist.

## Current state

| Area | Status | Main remaining gap |
| --- | --- | --- |
| Renderer build | Vite builds the React renderer. | Bundle size and legacy browser compatibility shims. |
| Desktop shell | Tao/Wry shell is active on Linux. | Real macOS/Windows validation and production shutdown/package evidence. |
| Headless bridge | Authenticated loopback WebSocket bridge serves typed operations. | Security review, reconnect/load validation, and request-budget review. |
| Git operations | Measured renderer-driven Git command sites are zero. | Hook environment, trampoline ownership, progress parity, and final shim removal. |
| Assisted commit | Typed host operations perform Git object/index/commit work. | Transaction ownership still depends on the browser-incomplete async-context design. |
| Copilot | Rust runtime client exists; renderer still owns parts of the SDK client lifecycle. | Live Rust-client turn validation, model/quota operations, packaged runtime layout. |
| GitHub/API | Some typed operations exist. | Broad PR/issue/check/enterprise/pagination/cancellation migration. |
| Credentials | Rust credential broker exists for scoped Git operations. | Account/keychain and remaining `keytar` shim flows. |
| Packaging | Rust/Vite development package exists. | Relocated smoke, signing, notarization, update channel, license inventory. |
| Tests | Vitest is the only TypeScript runner and uses the Rust backend. | Full-suite interference isolation, reusable repository fixture, lifetime policy. |

## Measured baseline

Measured on 2026-11-26 and enforced by `pnpm migration:check`:

- Renderer `transportKind` fallback checks: **0 in 0 files**
- Renderer-driven Git command sites: **0 in 0 files**
- Renderer imports of `dugite`, `fs`, `child_process`, `os`, `crypto`, and
  `keytar`: **0**
- TypeScript test files outside Vitest: **0**

These measurements do not mean migration is complete. They exclude internal
compatibility shims, host crates, test helpers, and workflows that still need
runtime validation.

## Important open contracts

- Assisted-commit ownership must move from Node-style async context propagation
  to explicit Rust transaction ownership before the async-hooks shim can be removed.
- Remote discovery may preserve configured URLs exactly, while typed transport
  operations must independently reject unsafe or credential-bearing destinations.
- Missing alternate object directories are strict during push but non-fatal for
  status and run protection.
- Rebase and cherry-pick still need live progress parity.
- Development packages are not production installers and must not replace a
  stable GitHub Desktop installation.

## Evidence expectations

A status change is credible only with current runtime evidence. Use focused
Vitest/Rust tests plus the validation gate in `rust/MIGRATION.md`. Do not copy
historical one-off session results into this file unless they are reproducible
from the current checkout.
