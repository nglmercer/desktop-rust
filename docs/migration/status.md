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
| Git operations | Measured renderer-driven Git command sites are zero. | Hook environment, trampoline ownership, and final shim removal. |
| Assisted commit | Typed host operations perform Git object/index/commit work. | Transaction ownership still depends on the browser-incomplete async-context design. |
| Copilot | Rust runtime client exists; renderer still owns parts of the SDK client lifecycle. | Live Rust-client turn validation, model/quota operations, packaged runtime layout. |
| GitHub/API | Some typed operations exist. | Broad PR/issue/check/enterprise/pagination/cancellation migration. |
| Credentials | Rust credential broker exists for scoped Git operations. | Account/keychain and remaining `keytar` shim flows. |
| Packaging | Rust/Vite development package exists. | Relocated smoke, signing, notarization, update channel, license inventory. |
| Tests | Vitest is the only TypeScript runner and uses the Rust backend. | Full-suite interference isolation and backend lifetime policy. |

## Measured baseline

The enforced numeric baseline lives in `rust/migration-baseline.json`; use
`pnpm migration:status` for current values rather than copying them here.

- Renderer `transportKind` fallback checks and renderer-driven Git command
  sites are zero.
- Renderer imports of `dugite`, `os`, `crypto`, and `keytar` are zero.
- Renderer imports of `fs`, `child_process`, `async_hooks`, `net`, `path`, and
  `registry-js` remain nonzero.
- Vitest runs every TypeScript test, but some test files still import the
  `node:test` compatibility API.

These measurements do not mean migration is complete. They exclude the internal
implementation of compatibility shims, host crates, test helpers, and workflows
that still need runtime validation.

## Important open contracts

- Assisted-commit ownership must move from Node-style async context propagation
  to explicit Rust transaction ownership before the async-hooks shim can be removed.
- Remote discovery may preserve configured URLs exactly, while typed transport
  operations must independently reject unsafe or credential-bearing destinations.
- Missing alternate object directories are strict during push but non-fatal for
  status and run protection.
- Development packages are not production installers and must not replace a
  stable GitHub Desktop installation.

## Evidence expectations

A status change is credible only with current runtime evidence. Use focused
Vitest/Rust tests plus the validation gate in `rust/MIGRATION.md`. Do not copy
historical one-off session results into this file unless they are reproducible
from the current checkout.
