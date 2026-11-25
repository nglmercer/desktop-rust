# Working with packages

The root renderer/build workspace uses pnpm. The pinned version is in
`packageManager` in `package.json` (currently pnpm 11.17.0). Rust dependencies
use Cargo and `Cargo.lock`.

Build dependencies live in the root `package.json`; application runtime
dependencies live in `app/package.json`, which is part of the pnpm workspace.
The retired nested npm lockfile must not be recreated; release versioning now
updates `app/package.json` directly.

## Install dependencies

```sh
git submodule update --init --recursive
pnpm install --frozen-lockfile --ignore-scripts
```

Use `--ignore-scripts` for normal development and CI bootstrap. Native helpers
needed by tests are provided through the Rust/web shims or built explicitly by
their own commands; dependency install hooks must not be required for the Rust
shell.

## Add or update packages

Use pnpm from the repository root:

```sh
pnpm add <package>
pnpm add -D <package>
pnpm add --filter desktop <package>   # application dependency
```

Commit `package.json` and `pnpm-lock.yaml` together.

Prefer versions published at least several days ago. The repository's pnpm and
npm settings deliberately preserve the existing dependency graph, disable
implicit peer installation, and avoid relying on package install scripts.

## Remove packages

```sh
pnpm remove <package>
```

After removal, search for imports, scripts, editor settings and documentation
references before deleting the lockfile changes or obsolete configuration.

## Run scripts and tools

Use `pnpm <script>` or `pnpm run <script>`:

```sh
pnpm test -- app/test/unit/repository-test.ts
pnpm lint
pnpm exec tsc --noEmit -p tsconfig.json
```

Release scripts update `app/package.json` directly and do not require the npm
CLI for version bumps.
