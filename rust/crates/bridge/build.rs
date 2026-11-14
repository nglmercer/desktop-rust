#[path = "../vite_build.rs"]
mod vite_build;

fn main() {
    println!("cargo:rerun-if-env-changed=DESKTOP_VITE_DIST");
    if std::env::var_os("CARGO_FEATURE_EMBEDDED_UI").is_some() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let dist = match std::env::var_os("DESKTOP_VITE_DIST") {
            Some(value) => vite_build::explicit_bundle(&repo, value),
            None => repo
                .join("out/vite-dist")
                .canonicalize()
                .expect("build the renderer before embedding it"),
        };
        vite_build::export_bundle(&dist);
    }
}
