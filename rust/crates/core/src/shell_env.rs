//! Host-owned capture of the login-shell environment used for Git hooks.
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const OUTPUT_LIMIT: usize = 1024 * 1024;
const SHELL_KINDS: [&str; 4] = ["git-bash", "pwsh", "powershell", "cmd"];

fn validate_shell_kind(shell_kind: Option<&str>) -> Result<()> {
    if let Some(kind) = shell_kind {
        anyhow::ensure!(SHELL_KINDS.contains(&kind), "unsupported hooks shell");
    }
    Ok(())
}

/// Capture the shell environment without renderer-controlled executables.
///
/// Unix is implemented first: the host executes itself in its private
/// `__printenvz` mode through the user's login shell. Windows keeps using the
/// legacy renderer path until its registry/shell discovery is migrated.
pub async fn renderer_shell_env(cwd: Option<String>, shell_kind: Option<String>) -> Result<Value> {
    validate_shell_kind(shell_kind.as_deref())?;
    renderer_shell_env_platform(cwd, shell_kind).await
}

#[cfg(unix)]
async fn renderer_shell_env_platform(
    cwd: Option<String>,
    shell_kind: Option<String>,
) -> Result<Value> {
    use std::ffi::OsString;
    use std::io::ErrorKind;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::process::Stdio;
    use tokio::io::AsyncReadExt;

    fn quote(value: &[u8]) -> Vec<u8> {
        let mut quoted = vec![b'\''];
        for byte in value {
            if *byte == b'\'' {
                quoted.extend_from_slice(b"'\\''");
            } else {
                quoted.push(*byte);
            }
        }
        quoted.push(b'\'');
        quoted
    }

    fn shell_command(executable: &Path) -> Result<OsString> {
        let mut command = quote(executable.as_os_str().as_bytes());
        command.push(b' ');
        command.extend_from_slice(&quote(b"__printenvz"));
        Ok(OsString::from_vec(command))
    }

    fn cwd_path(cwd: Option<String>) -> Result<PathBuf> {
        let path = match cwd {
            Some(value) => {
                anyhow::ensure!(
                    !value.is_empty() && !value.contains('\0'),
                    "Invalid hooks shell directory"
                );
                let path = PathBuf::from(value);
                if path.is_absolute() {
                    path
                } else {
                    std::env::current_dir()?.join(path)
                }
            }
            None => std::env::current_dir()?,
        };
        anyhow::ensure!(path.is_dir(), "Hooks shell directory does not exist");
        Ok(path)
    }

    async fn drain(
        mut reader: impl tokio::io::AsyncRead + Unpin,
        keep: usize,
    ) -> std::io::Result<(Vec<u8>, bool)> {
        let mut output = Vec::new();
        let mut overflow = false;
        let mut chunk = [0_u8; 8192];
        loop {
            let count = reader.read(&mut chunk).await?;
            if count == 0 {
                return Ok((output, overflow));
            }
            let remaining = keep.saturating_sub(output.len());
            if remaining > 0 {
                output.extend_from_slice(&chunk[..count.min(remaining)]);
            }
            overflow |= count > remaining;
        }
    }

    let cwd = cwd_path(cwd)?;
    let executable = std::env::current_exe()?.canonicalize()?;
    let shell = std::env::var_os("SHELL")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from("/bin/sh"));
    let mut command = tokio::process::Command::new(&shell);
    command
        .args([OsString::from("-ilc"), shell_command(&executable)?])
        .current_dir(&cwd)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(json!({"kind":"failure","shell_kind":shell_kind}));
        }
        Err(error) => return Err(error).context("start hooks shell"),
    };
    let stdout = child.stdout.take().context("capture hooks shell stdout")?;
    let stderr = child.stderr.take().context("capture hooks shell stderr")?;
    let ((stdout, stdout_overflow), (_stderr, _), status) = tokio::try_join!(
        drain(stdout, OUTPUT_LIMIT),
        drain(stderr, 64 * 1024),
        child.wait()
    )?;
    anyhow::ensure!(!stdout_overflow, "hooks shell output is too large");
    if !status.success() {
        bail!("hooks shell exited unsuccessfully");
    }

    let text = String::from_utf8_lossy(&stdout);
    let start = "--printenvz--begin\n";
    let end = "\n--printenvz--end\n";
    let start_at = text
        .find(start)
        .map(|index| index + start.len())
        .context("could not find start marker in shell output")?;
    let end_at = text
        .rfind(end)
        .context("could not find end marker in shell output")?;
    anyhow::ensure!(
        start_at <= end_at,
        "could not find end marker in shell output"
    );
    let mut environment = serde_json::Map::new();
    for entry in text[start_at..end_at].split('\0') {
        if entry.is_empty() {
            continue;
        }
        if let Some((name, value)) = entry.split_once('=')
            && !name.is_empty()
        {
            environment.insert(name.to_owned(), Value::String(value.to_owned()));
        }
    }
    Ok(json!({"kind":"success","env":Value::Object(environment)}))
}

#[cfg(not(unix))]
async fn renderer_shell_env_platform(
    _cwd: Option<String>,
    shell_kind: Option<String>,
) -> Result<Value> {
    Ok(json!({"kind":"failure","shell_kind":shell_kind}))
}

#[cfg(test)]
mod tests {
    #[test]
    fn validates_shell_names() {
        assert!(super::validate_shell_kind(None).is_ok());
        assert!(super::validate_shell_kind(Some("git-bash")).is_ok());
        assert!(super::validate_shell_kind(Some("sh")).is_err());
    }
}
