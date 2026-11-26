# Tooling support for GitHub Desktop

## Editor

The repository is configured for [Visual Studio Code](https://code.visualstudio.com/):

1. Open the Extensions view.
2. Choose **Show Workspace Recommended Extensions**.
3. Install the Biome and oxlint extensions.

The workspace settings use Biome as the default formatter. TypeScript editor
support uses `@typescript/compiler-api` because TypeScript 7 currently ships
the native compiler (`tsc`) without the legacy language-service/compiler API.

## Command-line tools

```sh
pnpm lint           # oxlint + formatting check
pnpm check:types    # TypeScript 7 application typecheck
pnpm compile:script # TypeScript 7 script typecheck
pnpm test           # Vitest unit tests
cargo test          # Rust tests
```

TypeScript 7 is the `typescript` package and the `tsc` binary in
`node_modules/.bin`. `@typescript/compiler-api` remains installed side-by-side
only for code that imports the old compiler API (`script/ts-register.cjs`,
native-module emission tests, usage-data introspection, and the branch-rules
oracle). Do not add new compiler-API consumers; use the TypeScript 7 CLI for
normal checking.

## Debugging the desktop shell

```sh
git submodule update --init --recursive
pnpm install --frozen-lockfile --ignore-scripts
pnpm start
```

The launcher builds the Rust host and watches the Vite renderer. Use the
webview developer tools in the running app to inspect the renderer, console,
network activity and React component tree. After a renderer rebuild, reload
with <kbd>Ctrl/Command+Alt+R</kbd>.

React Dev Tools remains useful for component and performance investigation.
For Rust-side logging, run the shell or CLI from a terminal so stderr remains
visible.
