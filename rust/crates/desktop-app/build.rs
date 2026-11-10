//! The renderer bundle is embedded with `include_dir!` for release, but
//! served from disk in debug (`bridge.rs`) so hashed Vite filenames
//! never go stale between `pnpm vite:build` and `cargo run`.
//!
//! To keep `cargo run -p desktop-app` fully automatic, this script
//! builds the Vite bundle itself when it is missing (or `DESKTOP_VITE_FORCE=1`).
//! If the build tools are unavailable it panics with the manual command.

use std::path::Path;
use std::process::Command;

fn run_vite(repo_root: &Path) -> bool {
    // Try pnpm, then npm, then npx vite.
    for (prog, args) in [
        ("pnpm", &["vite:build"][..]),
        ("npm", &["run", "vite:build"][..]),
        ("npx", &["vite", "build"][..]),
    ] {
        println!("cargo:warning=running `{prog} {}` for renderer bundle", args.join(" "));
        let status = Command::new(prog)
            .args(args)
            .current_dir(repo_root)
            .status()
            .ok()
            .filter(|s| s.success());
        if status.is_some() {
            // Build highlighter worker on its own config if `vite:build`
            // already did it via npm script, otherwise run it again.
            let highlighter = repo_root.join("out/vite-dist/highlighter.js");
            if !highlighter.is_file() {
                let _ = Command::new("npx")
                    .args(["vite", "build", "-c", "vite.highlighter.config.ts"])
                    .current_dir(repo_root)
                    .status();
            }
            return highlighter.is_file();
        }
    }
    false
}

fn main() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest.join("../../..");
    let dist = repo_root.join("out/vite-dist");
    println!("cargo:rerun-if-changed={}", dist.display());
    // Re-run if Vite inputs change, so a new hash triggers a rebuild.
    for p in ["vite.config.ts", "vite.highlighter.config.ts", "app/index.html"] {
        println!("cargo:rerun-if-changed={}", repo_root.join(p).display());
    }
    println!("cargo:rerun-if-env-changed=DESKTOP_VITE_FORCE");
    println!("cargo:rerun-if-env-changed=DESKTOP_SERVE_FROM_DISK");

    let force = std::env::var("DESKTOP_VITE_FORCE").is_ok();
    let need_build = force || !dist.join("index.html").is_file() || !dist.join("highlighter.js").is_file();
    if need_build {
        if !run_vite(&repo_root) {
            panic!(
                "the renderer bundle is missing at {}.\n\
                 Could not build it automatically. Build it from the repository root first:\n\
                 pnpm install && pnpm vite:build\n\
                 (which runs `vite build` and `vite build -c vite.highlighter.config.ts`)\n\
                 Debug builds serve from disk, so `pnpm vite:build` once is enough;\n\
                 subsequent `cargo run` will pick up fresh hashed filenames without a rebuild.",
                dist.display()
            );
        }
    }
}
