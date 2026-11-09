//! Single-instance handoff.
//!
//! The OAuth callback and "Open in Desktop" links arrive as a *new* process
//! launched by the OS with the URL as an argument. That process must hand the
//! URL to the window that is already running (it holds the pending sign-in)
//! and exit, as Electron's `requestSingleInstanceLock` did.

use std::path::Path;

/// Arguments worth forwarding to a running instance.
pub fn is_forwardable(arg: &str) -> bool {
    desktop_core::app::is_app_url(arg)
        || arg.starts_with("--cli-open=")
        || arg.starts_with("--cli-clone=")
}

#[cfg(unix)]
mod imp {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;
    use std::time::Duration;

    /// Hand `args` to a running instance. Returns `false` when there is none.
    pub fn forward(socket: &Path, args: &[String]) -> bool {
        let Ok(mut stream) = UnixStream::connect(socket) else {
            return false;
        };
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let Ok(mut line) = serde_json::to_string(args) else {
            return false;
        };
        line.push('\n');
        if stream.write_all(line.as_bytes()).is_err() {
            return false;
        }
        // Wait for the acknowledgement so we only exit once it was received.
        let mut ack = String::new();
        BufReader::new(stream).read_line(&mut ack).is_ok() && ack.trim() == "ok"
    }

    /// Become the primary instance and call `on_args` for each handoff.
    pub fn listen(socket: &Path, on_args: impl Fn(Vec<String>) + Send + 'static) -> std::io::Result<()> {
        // `forward` failed, so any existing socket file is a stale leftover.
        let _ = std::fs::remove_file(socket);
        let listener = UnixListener::bind(socket)?;
        // Only this user may inject launch arguments.
        std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;

        std::thread::Builder::new()
            .name("desktop-instance".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                    let mut line = String::new();
                    if BufReader::new(&stream).read_line(&mut line).is_err() {
                        continue;
                    }
                    match serde_json::from_str::<Vec<String>>(line.trim()) {
                        Ok(args) => {
                            let _ = (&stream).write_all(b"ok\n");
                            on_args(args);
                        }
                        Err(error) => tracing::warn!(%error, "ignoring malformed instance handoff"),
                    }
                }
            })?;
        Ok(())
    }
}

#[cfg(not(unix))]
mod imp {
    use std::path::Path;

    // Not implemented for this platform yet: every launch opens its own window.
    pub fn forward(_socket: &Path, _args: &[String]) -> bool {
        false
    }
    pub fn listen(_socket: &Path, _on_args: impl Fn(Vec<String>) + Send + 'static) -> std::io::Result<()> {
        Ok(())
    }
}

pub fn forward(socket: &Path, args: &[String]) -> bool {
    imp::forward(socket, args)
}

pub fn listen(socket: &Path, on_args: impl Fn(Vec<String>) + Send + 'static) -> std::io::Result<()> {
    imp::listen(socket, on_args)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn a_second_launch_hands_its_arguments_to_the_first() {
        let socket = std::env::temp_dir().join(format!("desktop-instance-test-{}.sock", std::process::id()));
        let args = vec!["x-github-desktop-dev-auth://oauth?code=c&state=s".to_string()];

        assert!(!forward(&socket, &args), "nothing is listening yet");

        let (sender, receiver) = mpsc::channel();
        listen(&socket, move |received| sender.send(received).unwrap()).unwrap();

        assert!(forward(&socket, &args));
        assert_eq!(receiver.recv_timeout(Duration::from_secs(5)).unwrap(), args);
        std::fs::remove_file(&socket).unwrap();
    }

    #[test]
    fn only_urls_and_cli_actions_are_forwarded() {
        assert!(is_forwardable("x-github-desktop-dev-auth://oauth?code=c&state=s"));
        assert!(is_forwardable("--cli-open=/tmp/repo"));
        assert!(!is_forwardable("--self-test"));
        assert!(!is_forwardable("https://example.com"));
    }
}
