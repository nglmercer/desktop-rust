"""Validate Linux native smoke-test launch uses a headless-stable ordering."""
import re
import sys
from pathlib import Path

WORKFLOW = Path(__file__).resolve().parents[2] / ".github" / "workflows" / "rust.yml"

def main() -> None:
    text = WORKFLOW.read_text(encoding="utf-8")

    # The install step must provide a portal and software-rendering stack.
    for pkg in [
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
    ]:
        assert pkg in text, f"missing apt package {pkg}"

    # The smoke-test env must force x11 + software rendering and runtime dir.
    for key, value in [
        ("WEBKIT_DISABLE_COMPOSITING_MODE", "'1'"),
        ("WEBKIT_DISABLE_DMABUF_RENDERER", "'1'"),
        ("GDK_BACKEND", "x11"),
        ("LIBGL_ALWAYS_SOFTWARE", "'1'"),
        ("GDK_SYNCHRONIZE", "'1'"),
        ("XDG_RUNTIME_DIR", "xdg-runtime"),
    ]:
        assert key in text, f"missing env {key}"
        assert value in text, f"missing env value for {key}: {value}"

    # xvfb-run must wrap dbus-run-session, not the other way around.
    # Previously "dbus-run-session -- xvfb-run" started dbus before DISPLAY
    # existed, so the Gtk portal failed with "cannot open display:".
    m = re.search(r"xvfb-run.*?dbus-run-session", text, re.DOTALL)
    assert m, "expected 'xvfb-run ... dbus-run-session' ordering"

    bad = "dbus-run-session -- xvfb-run"
    assert bad not in text, f"regression: found bad ordering {bad!r}"

    # The smoke binary path is pinned by .cargo/config.toml target-dir.
    assert "rust/target/debug/desktop-native" in text
    assert "--smoke-test" in text

    # timeout stays as the innermost wrapper around the binary.
    assert "dbus-run-session -- timeout 60s" in text

    print("PASS: native smoke launch order, env, and deps are headless-stable")

if __name__ == "__main__":
    main()
