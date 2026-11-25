# Linting and formatting

The fast JavaScript/TypeScript gate uses:

- [oxlint](https://oxc.rs/docs/guide/usage/linter.html) for static analysis,
  including the project rules in `oxlint-rules/`.
- [Biome](https://biomejs.dev/) for formatting. Biome's linter is disabled in
  the root configuration; oxlint is the only JavaScript linter.
- [markdownlint](https://github.com/DavidAnson/markdownlint) for Markdown when
  documentation is changed.

The editor integrations recommended by the repository are the Biome and oxlint
VS Code extensions. See [tooling](./tooling.md).

## Running checks

```sh
pnpm lint
```

This runs oxlint and then checks formatting with Biome. On a warm checkout the
command should complete in a few seconds.

Useful focused commands:

```sh
pnpm lint:oxlint      # oxlint only
pnpm lint:security    # only the project security rules
pnpm format:check     # Biome formatting only
pnpm markdownlint     # Markdown documentation only
```

## Fixing issues

```sh
pnpm lint:fix
```

This applies oxlint's safe fixes and formats the repository with Biome. Review
the resulting diff before committing; format migration is intentionally
repo-wide so generated or vendored exclusions live in `biome.json`.

## Project rules

`oxlint-rules/desktop.mjs` implements the project-specific checks formerly
implemented as ESLint/Biome plugins:

- `desktop/insecure-random`
- `desktop/no-loosely-typed-webcontents-ipc`
- `desktop/react-no-unbound-dispatcher-props`
- `desktop/react-readonly-props-and-state`
- `desktop/react-proper-lifecycle-methods`

Their regression tests live in `oxlint-rules/tests/` and run with:

```sh
pnpm test:oxlint
```

The oxlint JavaScript plugin API is still marked upstream as experimental, so
keep the rules syntax-only and covered by fixtures. Type-aware checking belongs
in TypeScript, not in these plugins.

## Intentional migration gaps

Oxlint does not yet implement every legacy ESLint rule. In particular, the old
member-ordering, naming-convention, `react/jsx-no-bind`, prop-types, and some
JSDoc rules have no direct oxlint equivalent. The legacy `curly`,
`no-sync`, and explicit-member-accessibility gates currently have a large
inherited violation backlog and are disabled in `.oxlintrc.json` rather than
being silently reported forever. They remain style expectations in
`styleguide.md`; do not use that backlog as a reason to add new violations.
