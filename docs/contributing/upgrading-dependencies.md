# Upgrading Dependencies

This document outlines how we manage the dependencies that GitHub Desktop needs,
as some need to be handled differently to others

## General Guidelines

In the interest of stability and caution we tend to stay a version (or more) behind on our most critical dependencies. We vary whether this is a major, minor, or patch version depending on the development practices of the dependency's project.

| Dependency | Versions Behind Latest |
| --- | --- |
| typescript | >= 1 minor |
| codemirror | >= 1 minor |
| react | >= 1 minor |
| react-dom | >= 1 minor |
| keytar | >= 1 minor |

## The Impact Of A Dependency

We group the dependencies in `package.json` and `app/package.json` into three
buckets, based on their value to the project and what happens when a bad upgrade
occurs.

Because of the nature of the Node ecosystem, changes upgrading between versions
of a package can introduce subtle changes that need to be reviewed and vetted.
We also have finite engineering resources on the project, so we're a bit choosy
with how and when we choose to upgrade dependencies, and some dependencies might
be very stale for a very long period of time.

The measured impact of a dependency can be identified by answering these three
questions, and some examples of answers here will help with the next section.

### How is this dependency used in the project?

  - Core dependencies like React and Typescript are critical to
    Desktop - without these dependencies we'd have a very different product
  - Application dependencies are other external packages which are needed at
    runtime - how can we test they still work as expected?
  - Tooling dependencies help to build and verify Desktop works as expected, but
    aren't shipped to users

### If we ship a bad update due to updating a dependency, what is the impact to users?

  - If a core dependency is broken, it may cause the app to crash, lose the
    user's work, or require the user to reinstall an old version of Desktop to
    get back to a usable state
  - An application dependency breaking may affect some user's workflows and
    their ability to work
  - Broken tooling impacts the developers and their ability to work on the
    project, and potentially affects shipping builds to users, but otherwise
    might not be seen

### Can we leverage automation to verify a change is safe?

  - Core dependencies need to be tested in a variety of setups due to their
    complexity, and may still require manual testing in some cases even with
    automated tests in place
  - Application dependencies may be able to be wrapped in tests, to help
    identify regressions before a change is released to users
  - Tooling dependencies are used heavily as part of continuous integration
    tests, and any potential problems will be caught early

## Grouping dependencies by impact

With the criteria above, we can group dependencies into these three buckets that
help guide when they should be updated.

### Core dependencies

These are the most important dependencies to the app, and include:

 - `package.json`
   - `@types/node`
   - `typescript`
   - `vite` and related dependencies
 - `app/package.json`
   - `codemirror`
   - `dugite`
   - `react`
   - `react-dom`
   - `keytar`

Anyone who wants to get involved with updating these versions should be very
familiar with both the library and what has changed between versions, so the
review process is focused on verifying the impact of the change rather than
understanding what has changed.

The maintainers have significant experience with these dependencies, and may
wish to upgrade one of these dependencies at a later time than when a
contribution is opened by the community.

### Application dependencies

This group of dependencies represent the rest of the listed dependencies in
`app/package.json` - there are lots of small dependencies which are approachable
for external contributions.

If you are interested in upgrading one of these, take a look at where it is
used in the application and confirm that the affected functionality works as
expected before opening a pull request.

### Tooling dependencies

The remaining dependencies in `package.json` are used to help build, test and
verify Desktop, and are covered by a strong suite of tests which our CI
infrastructure will run.

External contributors who have some familiarity with the libraries that we use
are encouraged to contribute, but please ensure that PRs focus on upgrading a
specific set of dependencies - this will make reviews easier to perform.

## TypeScript upgrades

TypeScript 7 is the command-line type checker through the `@typescript/native`
package alias. TypeScript 6 remains installed as `typescript` only for the
legacy compiler API used by `script/ts-register.cjs`, native-module emission
tests, usage-data introspection, and the branch-rules oracle.

When upgrading either compiler:

- keep the root aliases and the TypeScript-based packages under `vendor/`
  intentionally aligned with the API they consume;
- preserve strictness and library declaration checking;
- update incompatible dependencies or callers rather than disabling checks;
- do not add new consumers of the TypeScript 6 compiler API.

Vite compiles the application and syntax-highlighting worker using `bundler`
module resolution. Build scripts use `nodenext`, and native modules use `node16`
to retain their CommonJS output. Each compiler project declares its source root
explicitly. Use public package exports when importing types; for SDK types that
are not exported directly, derive them from the public API instead of importing
private implementation paths.

Validate all compiler projects, renderer builds, unit and script suites, and
custom oxlint rule tests:

```shell
pnpm check:types
pnpm compile:script
pnpm exec tsc -p app/src/highlighter --noEmit
pnpm vite:build
pnpm test
pnpm test:script
pnpm test:oxlint
pnpm lint
```

For compiler-API compatibility, additionally run the branch-rules oracle
typecheck in `rust/tests/branch-rules-oracle` and the script tests that exercise
`script/ts-register.cjs` and native-module output.

Also run the native-module build/test commands for each TypeScript-based package
under `vendor/` when their compiler or emitted module format changes.

Check emitted paths, not only successful compilation: a `rootDir` change can
move output away from a package's published entry points. For example,
`desktop-notifications` uses `lib/` as its source root to emit `dist/index.js`,
not `dist/lib/index.js`. The native-module output tests in `pnpm test:script`
capture compiler output in memory so stale build files cannot hide this
regression.

## DefinitelyTyped dependencies

We need to specifically call out the `@types/*` dependencies in `package.json`
because those are typically updated when a related package is updated.

These are the rough rules for handling `@types/*` dependency updates, using
`keytar` as an example as it has a corresponding type declaration package
`@types/keytar`:

 - if you are upgrading `keytar`, ensure that you also upgrade `@types/keytar`
   and that any code affected by the type declarations change is also updated
 - if there is no `keytar` upgrade but a new version of `@types/keytar` is
   available, this suggests that some declarations have been updated - ensure
   that the app code works fine with it

Not all libraries will have a corresponding `@types/` dependency, but as we use
TypeScript heavily in the codebase we want to ensure we're working against the
latest declarations of the untyped JS that we consume.
