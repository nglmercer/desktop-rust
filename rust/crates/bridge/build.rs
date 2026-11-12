fn main() {
    if std::env::var_os("CARGO_FEATURE_EMBEDDED_UI").is_some() {
        println!("cargo:rerun-if-changed=../../../out/vite-dist");
    }
}
