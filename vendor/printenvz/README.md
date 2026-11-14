# printenvz

This helper prints the current process environment using the marker and
NUL-delimited format consumed by Desktop's hook environment loader. It is
implemented in Rust and preserves bytes in valid environment name/value pairs on
Unix, including values that are not UTF-8. Empty-name entries are omitted.

The package exports `getPrintenvzPath()`, which returns the installed executable
at `build/Release/printenvz` (or `printenvz.exe` on Windows). Existing builds
copy that executable into the application output directory.

The Electron npm setup and build explicitly compile this helper using Cargo;
dependency installation does not run an implicit build script. The installer
uses npm's requested `cpu` or `arch` and the packaging `TARGET_ARCH` when
provided, and fails if their values disagree, the target OS differs from the
host, or the required Rust target is unavailable. It preserves the native host
triple for native builds and supports only known cross-target combinations. A
failed build leaves the existing executable untouched. A Rust toolchain and the
requested target must be installed. On Windows and macOS, the new Rust build and
cross-architecture installations still need native runtime verification.

Run the Rust protocol tests with:

```sh
cargo test --manifest-path vendor/printenvz/Cargo.toml --locked
```
