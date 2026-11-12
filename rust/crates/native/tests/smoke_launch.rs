//! Guard the headless Linux webview smoke-test configuration.

const WORKFLOW: &str = include_str!("../../../../.github/workflows/rust.yml");

#[test]
fn linux_smoke_launch_has_required_dependencies_environment_and_order() {
    for package in [
        "libwebkit2gtk-4.1-dev",
        "libgtk-3-dev",
        "xvfb",
        "dbus-x11",
        "xdg-desktop-portal",
        "xdg-desktop-portal-gtk",
        "at-spi2-core",
        "libgl1-mesa-dri",
        "mesa-utils",
        "libegl1-mesa",
        "libegl1",
    ] {
        assert!(WORKFLOW.contains(package), "missing apt package {package}");
    }

    for (key, value) in [
        ("WEBKIT_DISABLE_COMPOSITING_MODE", "'1'"),
        ("WEBKIT_DISABLE_DMABUF_RENDERER", "'1'"),
        ("GDK_BACKEND", "x11"),
        ("LIBGL_ALWAYS_SOFTWARE", "'1'"),
        ("GDK_SYNCHRONIZE", "'1'"),
        ("XDG_RUNTIME_DIR", "xdg-runtime"),
    ] {
        assert!(WORKFLOW.contains(key), "missing environment key {key}");
        assert!(WORKFLOW.contains(value), "missing value {value} for {key}");
    }

    let xvfb = WORKFLOW.find("xvfb-run").expect("missing xvfb-run");
    assert!(
        WORKFLOW[xvfb..].contains("dbus-run-session"),
        "xvfb-run must wrap dbus-run-session"
    );
    assert!(!WORKFLOW.contains("dbus-run-session -- xvfb-run"));
    assert!(WORKFLOW.contains("rust/target/debug/desktop-native"));
    assert!(WORKFLOW.contains("--smoke-test"));
    assert!(WORKFLOW.contains("dbus-run-session -- timeout 60s"));
}
