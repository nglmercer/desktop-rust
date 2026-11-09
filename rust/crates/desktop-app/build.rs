//! The renderer bundle is embedded with `include_dir!`, which cargo does not
//! track: without this script a rebuilt UI is silently ignored and the binary
//! keeps serving the bundle it was last compiled with.

use std::path::Path;

fn main() {
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/vite-dist");
    println!("cargo:rerun-if-changed={}", dist.display());

    if !dist.join("index.html").is_file() {
        panic!(
            "the renderer bundle is missing at {}.\n\
             Build it from the repository root first: pnpm install && pnpm vite:build",
            dist.display()
        );
    }
}
