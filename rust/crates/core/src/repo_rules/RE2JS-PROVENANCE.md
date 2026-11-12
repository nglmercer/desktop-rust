# Trusted RE2JS matcher provenance

`re2js.js` is generated with Vite from the matcher and operator enum extracted
from the current `app/src/lib/helpers/repo-rules.ts` and `app/src/lib/api.ts`,
plus the pinned ESM distribution of RE2JS 2.0.1. The matcher preserves the
repository's MIT-licensed source.
RE2JS's MIT license is included in `RE2JS-LICENSE.txt` and the generated banner.

The isolated development dependency is pinned in
`rust/tests/branch-rules-oracle/package-lock.json`, including registry integrity:
`sha512-X59sWAF3UaMKfsVZT5/kJk1kBK10artA4jxScW4dPYtIuyDAWITcTDgkgEf9GPaqCKMVPgL2oEJGib9jN4WnkQ==`.

The oracle runs in the Rust migration CI job and checks the current renderer's
installed RE2JS against the same frozen cases. It does not prove parity for
all possible patterns or versions.

```sh
npm ci --prefix rust/tests/branch-rules-oracle --ignore-scripts
node --import tsx rust/tests/branch-rules-oracle/build-engine.mts --check
```

The bundle exposes only `__desktopMatchRules(patternsJSON, branch)`, which
returns a JSON boolean array string. Its input is data passed as function
arguments, never JavaScript source. The host must impose memory/stack/time
limits. Runtime use requires no Node, CommonJS, module loader, browser APIs,
network, or OS integration. Engine exceptions remain errors; they must never
be converted into a passed policy. The source oracle and dialect corpus in the
tests-only package provide the matching/bypass reference contract.
