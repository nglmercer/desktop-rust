//! Scoped GitHub Copilot app handoff execution.
//!
//! The renderer chooses a validated installation, but the host owns process
//! construction, the fixed `open` argument shape, output bounds, and the
//! acknowledgement timeout. No shell is involved.
use anyhow::Result;
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use tokio::io::AsyncReadExt;

const OUTPUT_LIMIT: usize = 1024 * 1024;
const ACKNOWLEDGEMENT_TIMEOUT: Duration = Duration::from_secs(30);

fn outcome(kind: &str, message: impl Into<String>) -> Value {
    json!({ "kind": kind, "message": message.into() })
}

fn valid_path_input(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && !value.contains('\0')
        && Path::new(value).is_absolute()
}

async fn read_limited(
    reader: impl tokio::io::AsyncRead + Unpin,
    limit: usize,
) -> std::io::Result<Vec<u8>> {
    let mut reader = reader;
    let mut kept = Vec::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        let remaining = limit.saturating_sub(kept.len());
        if remaining > 0 {
            kept.extend_from_slice(&buffer[..count.min(remaining)]);
        }
    }
    Ok(kept)
}

#[cfg(unix)]
async fn kill_process_tree(child: &mut tokio::process::Child) {
    if let Some(id) = child.id() {
        // The child was started as a process-group leader. It still exists as
        // a zombie or live process here, so its process-group ID cannot have
        // been reused by an unrelated process.
        unsafe { libc::kill(-(id as i32), libc::SIGKILL) };
    }
    let _ = child.kill().await;
}

#[cfg(not(unix))]
async fn kill_process_tree(child: &mut tokio::process::Child) {
    let _ = child.kill().await;
}

async fn open_inner(executable: &str, repository_path: &str, timeout: Duration) -> Value {
    if !valid_path_input(executable) || !valid_path_input(repository_path) {
        return outcome(
            "launch-failed",
            "Copilot app handoff paths must be absolute",
        );
    }
    match std::fs::metadata(executable) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return outcome("not-found", "Copilot app executable is not a file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return outcome("not-found", "Copilot app executable was not found");
        }
        Err(error) => return outcome("launch-failed", error.to_string()),
    }
    match std::fs::metadata(repository_path) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return outcome("launch-failed", "Repository path is not a directory"),
        Err(error) => return outcome("launch-failed", error.to_string()),
    }

    let mut command = tokio::process::Command::new(executable);
    command
        .arg("open")
        .arg(repository_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return outcome("not-found", error.to_string());
        }
        Err(error) => return outcome("launch-failed", error.to_string()),
    };
    let stdout = child.stdout.take().expect("stdout pipe");
    let stderr = child.stderr.take().expect("stderr pipe");
    let stdout_task = tokio::spawn(read_limited(stdout, OUTPUT_LIMIT));
    let stderr_task = tokio::spawn(read_limited(stderr, OUTPUT_LIMIT));
    let (status, timed_out) = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => (status, false),
        Ok(Err(error)) => return outcome("launch-failed", error.to_string()),
        Err(_) => {
            kill_process_tree(&mut child).await;
            match child.wait().await {
                Ok(status) => (status, true),
                Err(error) => return outcome("launch-failed", error.to_string()),
            }
        }
    };
    let stdout = stdout_task
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    let stderr = stderr_task
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    if timed_out {
        return outcome(
            "launch-failed",
            "Copilot app did not acknowledge the handoff in time",
        );
    }
    if !status.success() {
        let detail = String::from_utf8_lossy(&stderr).trim().to_owned();
        let detail = if detail.is_empty() {
            String::from_utf8_lossy(&stdout).trim().to_owned()
        } else {
            detail
        };
        return outcome(
            "launch-failed",
            if detail.is_empty() {
                format!("Copilot app exited with status {status}")
            } else {
                detail
            },
        );
    }
    outcome("success", "")
}

/// Open a repository in the Copilot app and wait for CLI acknowledgement.
pub async fn open_copilot_app(executable: String, repository_path: String) -> Result<Value> {
    anyhow::ensure!(
        executable.len() <= 4096 && repository_path.len() <= 4096,
        "Copilot app handoff path is too large"
    );
    Ok(open_inner(&executable, &repository_path, ACKNOWLEDGEMENT_TIMEOUT).await)
}

#[cfg(all(test, unix))]
mod tests {
    use super::open_inner;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "desktop-copilot-app-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fixture() -> (Fixture, std::path::PathBuf, std::path::PathBuf) {
        let directory = Fixture::new();
        let executable = directory.0.join("copilot-test");
        let repository = directory.0.join("repository");
        std::fs::create_dir(&repository).unwrap();
        (directory, executable, repository)
    }

    #[tokio::test]
    async fn missing_executable_is_not_found() {
        let (_directory, executable, repository) = fixture();
        let value = open_inner(
            executable.to_str().unwrap(),
            repository.to_str().unwrap(),
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(value["kind"], "not-found");
    }

    #[tokio::test]
    async fn success_requires_cli_acknowledgement() {
        let (_directory, executable, repository) = fixture();
        std::fs::write(
            &executable,
            "#!/bin/sh\n[ \"$1\" = open ] && [ -d \"$2\" ]\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let value = open_inner(
            executable.to_str().unwrap(),
            repository.to_str().unwrap(),
            Duration::from_secs(2),
        )
        .await;
        assert_eq!(value["kind"], "success");
    }

    #[tokio::test]
    async fn nonzero_output_preserves_literal_repository_arguments() {
        let (_directory, executable, repository) = fixture();
        std::fs::write(
            &executable,
            r#"#!/bin/sh
printf '%s' "$2" >&2
exit 7
"#,
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let special = _directory.0.join("a repo;$(touch nope)&\"quotes\"");
        std::fs::create_dir(&special).unwrap();
        let value = open_inner(
            executable.to_str().unwrap(),
            special.to_str().unwrap(),
            Duration::from_secs(2),
        )
        .await;
        assert_eq!(value["kind"], "launch-failed");
        assert_eq!(value["message"], special.to_string_lossy().as_ref());
    }

    #[tokio::test]
    async fn hung_process_is_bounded() {
        let (_directory, executable, repository) = fixture();
        std::fs::write(&executable, "#!/bin/sh\nwhile :; do sleep 1; done\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let value = open_inner(
            executable.to_str().unwrap(),
            repository.to_str().unwrap(),
            Duration::from_millis(50),
        )
        .await;
        assert_eq!(value["kind"], "launch-failed");
    }
}
