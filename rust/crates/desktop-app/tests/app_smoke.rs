//! Starts the real shell with an isolated profile and a built renderer.
#![cfg(target_os = "linux")]

use std::{
    fs,
    io::{BufRead, BufReader, Read},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Sender},
    thread,
    time::{Duration, Instant},
};

struct TestApp {
    child: Child,
    profile: PathBuf,
}

impl Drop for TestApp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.profile);
    }
}

fn collect_lines(stream: impl Read + Send + 'static, sender: Sender<String>) {
    thread::spawn(move || {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
}

#[test]
#[ignore = "requires a display and a built Vite renderer"]
fn shell_starts_and_renderer_reports_no_errors() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    assert!(
        root.join("out/vite-dist/index.html").is_file(),
        "build the renderer with pnpm vite:build first"
    );
    assert!(
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some(),
        "a display is required; run under xvfb-run if needed"
    );
    let profile = std::env::temp_dir().join(format!("desktop-smoke-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&profile).unwrap();
    let binary = std::env::var_os("DESKTOP_APP_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_github-desktop-rs")));
    let child = Command::new(binary)
        .current_dir(root)
        .env("HOME", &profile)
        .env("XDG_DATA_HOME", profile.join("data"))
        .env("XDG_CONFIG_HOME", profile.join("config"))
        .env("XDG_CACHE_HOME", profile.join("cache"))
        .env("RUST_LOG", "info")
        .env("NO_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start the desktop application");
    let mut app = TestApp { child, profile };
    let (sender, receiver) = mpsc::channel();
    collect_lines(app.child.stdout.take().unwrap(), sender.clone());
    collect_lines(app.child.stderr.take().unwrap(), sender);
    let mut lines = Vec::new();
    let mut ready_at = None;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(line) = receiver.recv_timeout(Duration::from_millis(100)) {
            if line.contains("webview ready") && ready_at.is_none() {
                ready_at = Some(Instant::now());
            }
            lines.push(line);
        }
        assert!(
            app.child.try_wait().unwrap().is_none(),
            "application exited early: {lines:#?}"
        );
        assert!(
            Instant::now() < deadline || ready_at.is_some(),
            "webview did not start: {lines:#?}"
        );
        if ready_at.is_some_and(|time| time.elapsed() >= Duration::from_secs(8)) {
            break;
        }
    }
    lines.extend(receiver.try_iter());
    let failures: Vec<_> = lines
        .iter()
        .filter(|line| {
            line.contains("panicked")
                || (line.contains("[renderer")
                    && (line.contains("ERROR") || line.contains("error")))
        })
        .collect();
    assert!(
        failures.is_empty(),
        "application reported failures: {failures:#?}"
    );
}
