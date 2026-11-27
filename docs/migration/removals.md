# Migration removals

This file tracks what is already out of the active flow, which compatibility
pieces remain temporarily, and what evidence is required before deleting them.
Git history contains the detailed per-change ledger; this document only keeps
actionable current state.

## Removal rule

A removal is complete only when the replacement path is exercised at runtime.
Source counts, renamed files, or the existence of a typed Rust operation are not
enough. Where relevant, evidence must cover failure, cancellation, recovery,
progress, security, and byte-exact behavior.

## Already out of the active flow

- Electron main-process source, launchers, packagers, installers, type
  declarations, crash shell, and Electron-only test harnesses.
- Webpack and esbuild renderer builds and their exclusive dependencies.
- The root npm lockfile, `app/package-lock.json`, ESLint/Prettier tooling, and
  explicit Vitest suite routing.
- Python smoke/integration paths replaced by Rust or Vitest coverage.
- Renderer-driven Git command execution measured by `pnpm migration:status`;
  typed Rust host operations now own those command invocations.
- The broad `crypto-browserify` implementation behind commit links and the
  Copilot compatibility surface; scoped hashing and narrow compatibility code
  replaced it.
- Direct renderer `setImmediate`/`clearImmediate` use; a small MessageChannel
  scheduler now covers application callbacks and the narrow dependency surface.

## Temporary compatibility pieces

| Piece | Current state | Final removal condition |
| --- | --- | --- |
| `keytar` | Web shim/Rust-backed account flows remain. | No production consumer or test depends on the shim. |
| `registry-js` | Web shim remains for Windows shell/hook discovery paths. | Windows registry access is host-owned and consumers are gone. |
| `fs-admin` | Legacy compatibility surface remains only through migration artifacts. | No consumer or test depends on it. |
| `desktop-trampoline` | Real dependency and renderer credential/trampoline logic remain. | Git/SSH/askpass credential environments are served completely by Rust. |
| `printenvz` | Vendored helper remains for hook shell-environment capture. | Hook environment capture is Rust-owned or removed. |
| `dugite` | Real dependency remains for error tables, tests, and compatibility surfaces. | No runtime or test path needs the package directly. |
| `app/src/web/*` shims | Browser compatibility layer for renderer/dependencies. | Remove file by file after consumers and shim-specific tests are gone. |

## Structural removal candidates

| Candidate | Required before deletion |
| --- | --- |
| `rust/crates/desktop-shell-core-legacy` | Useful services are merged into `rust/crates/core`; GUI, CLI, and bridge no longer depend on it. |
| `rust/crates/native` as an independent host | The definitive shell is validated on supported platforms; any required executable compatibility is represented explicitly. |
| TypeScript persistence/domain projections | Rust is authoritative and TypeScript uses generated or narrow contracts. |
| Residual release scripts | Versioning, packaging, signing, updates, and license inventory are validated without them. |
| Stale migration documentation | Current concise documentation covers architecture, status, roadmap, and removal gates. |

## Preserved intentionally

React/Primer components, current styles and assets, syntax highlighting,
`gemoji`, `gitignore`, license templates, provenance notices, useful oracle
fixtures, and current tests are not legacy merely because they are old. Do not
remove them without a product or migration reason.

## Live evidence

Use `pnpm migration:status` for the numeric baseline and `rust/ROADMAP.md` for
current removal order. The detailed historical evidence for individual removals
lives in commit history and focused tests rather than in a second ledger here.
