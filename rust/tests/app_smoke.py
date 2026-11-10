"""Startup smoke test for the real desktop application.

Launches the built `github-desktop-rs` binary against a throwaway profile,
waits for the webview to come up, keeps it running for a few seconds, and
fails on a panic or on an error the renderer reported (its uncaught errors are
forwarded to the Rust log as `[ui]` lines).

It checks that the shell boots and that the renderer's React tree loads
without an uncaught error. It does not drive the UI: clicks, Git operations
and menus need a GUI driver, which this suite does not have.

Requires a built binary: `cargo build --manifest-path rust/Cargo.toml -p desktop-app`
and a renderer bundle: `pnpm vite:build`. Uses only the Python standard library.
"""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading
import time

REPO = Path(__file__).resolve().parents[2]
BINARY = Path(os.environ.get('DESKTOP_APP_BINARY', REPO / 'rust/target/debug/github-desktop-rs'))
READY = 'webview ready'
SETTLE_SECONDS = 8
BOOT_TIMEOUT_SECONDS = 60


def launch_command():
    # A display is required. Use a virtual one when none is available.
    if os.environ.get('DISPLAY') or os.environ.get('WAYLAND_DISPLAY'):
        return [str(BINARY)]
    if shutil.which('xvfb-run') is None:
        sys.exit('no display and no xvfb-run: cannot start the application')
    return ['xvfb-run', '-a', str(BINARY)]


def profile_environment(home: Path) -> dict:
    # Keep the test away from the developer's repositories, accounts and
    # single-instance socket.
    env = dict(os.environ)
    env.update({
        'HOME': str(home),
        'XDG_DATA_HOME': str(home / 'data'),
        'XDG_CONFIG_HOME': str(home / 'config'),
        'XDG_CACHE_HOME': str(home / 'cache'),
        'RUST_LOG': 'info',
        'NO_COLOR': '1',
    })
    return env


def main() -> int:
    if not BINARY.is_file():
        print(f'missing binary: {BINARY}', file=sys.stderr)
        return 2
    if not (REPO / 'out/vite-dist/index.html').is_file():
        print('missing renderer bundle: run `pnpm vite:build` first', file=sys.stderr)
        return 2

    home = Path(tempfile.mkdtemp(prefix='desktop-smoke-'))
    lines: list[str] = []
    ready = threading.Event()

    process = subprocess.Popen(
        launch_command(),
        cwd=str(REPO),
        env=profile_environment(home),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )

    def read() -> None:
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line.rstrip('\n'))
            if READY in line:
                ready.set()

    reader = threading.Thread(target=read, daemon=True)
    reader.start()

    try:
        if not ready.wait(BOOT_TIMEOUT_SECONDS):
            print('the webview did not come up', file=sys.stderr)
            print('\n'.join(lines[-40:]), file=sys.stderr)
            return 1
        time.sleep(SETTLE_SECONDS)
        if process.poll() is not None:
            print(f'the application exited early with {process.returncode}', file=sys.stderr)
            print('\n'.join(lines[-40:]), file=sys.stderr)
            return 1
    finally:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        reader.join(timeout=5)
        shutil.rmtree(home, ignore_errors=True)

    failures = [
        line
        for line in lines
        if 'panicked' in line or ('[renderer' in line and ('ERROR' in line or 'error' in line))
    ]
    if failures:
        print('the application reported failures:', file=sys.stderr)
        print('\n'.join(failures[:20]), file=sys.stderr)
        return 1
    print(f'ok: the application started, ran {SETTLE_SECONDS}s, and reported no renderer errors')
    return 0


if __name__ == '__main__':
    sys.exit(main())
