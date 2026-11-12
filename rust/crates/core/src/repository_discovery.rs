//! Read-only repository classification for the add and recovery workflows.
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};

pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

#[cfg(windows)]
pub(crate) fn display_canonical(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{unc}"))
    } else if let Some(local) = text.strip_prefix(r"\\?\") {
        PathBuf::from(local)
    } else {
        path
    }
}

#[cfg(not(windows))]
pub(crate) fn display_canonical(path: PathBuf) -> PathBuf {
    path
}

/// Classify an absolute directory without selecting it or changing its Git state.
pub async fn repository_type(path: &Path) -> Result<Value> {
    ensure!(path.is_absolute(), "repository path must be absolute");
    if !tokio::fs::metadata(path)
        .await
        .is_ok_and(|metadata| metadata.is_dir())
    {
        return Ok(json!({"kind":"missing"}));
    }
    let output = crate::process::repository_discovery(
        path,
        &crate::args(&[
            "rev-parse",
            "--is-bare-repository",
            "--show-cdup",
            "--git-dir",
        ]),
    )
    .await?;
    if output.code == 0 {
        if output.stdout.starts_with(b"true\n") {
            return Ok(json!({"kind":"bare"}));
        }
        if let Some(rest) = output.stdout.strip_prefix(b"false\n")
            && let Some(separator) = rest.iter().position(|byte| *byte == b'\n')
            && let Some(git_dir) = rest[separator + 1..].strip_suffix(b"\n")
        {
            let cdup = &rest[..separator];
            let root = normalize(&path.join(crate::os_from_bytes(cdup)?));
            let git_dir = normalize(&path.join(crate::os_from_bytes(git_dir)?));
            return Ok(
                json!({"kind":"regular", "topLevelWorkingDirectory":root.to_string_lossy(), "gitDir":git_dir.to_string_lossy()}),
            );
        }
    }
    let diagnostic = b"fatal: detected dubious ownership in repository at ";
    if output
        .stderr
        .windows(diagnostic.len())
        .any(|part| part == diagnostic)
    {
        let canonical = display_canonical(
            tokio::fs::canonicalize(path)
                .await
                .context("resolve unsafe repository path")?,
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        for candidate in canonical.ancestors() {
            let candidate = candidate.to_string_lossy();
            #[cfg(windows)]
            let candidate = candidate.replace('\\', "/");
            let message =
                format!("fatal: detected dubious ownership in repository at '{candidate}'\n");
            if stderr.starts_with(&message) || stderr.contains(&format!("\n{message}")) {
                return Ok(json!({"kind":"unsafe", "path":candidate}));
            }
        }
        bail!("Unable to determine the repository directory for the ownership exception.");
    }
    ensure!(
        matches!(output.code, 0 | 128),
        "repository discovery failed"
    );
    Ok(json!({"kind":"missing"}))
}

/// Add one existing canonical repository root to Git's global trust list.
pub async fn trust_repository(path: &Path) -> Result<Value> {
    ensure!(
        path.is_absolute(),
        "directory exception requires an absolute path"
    );
    let canonical = display_canonical(
        tokio::fs::canonicalize(path)
            .await
            .context("resolve directory exception")?,
    );
    let canonical_text = canonical
        .to_str()
        .context("directory exception path is not UTF-8")?;
    #[cfg(windows)]
    let normalized = canonical_text.replace('\\', "/");
    #[cfg(not(windows))]
    let normalized = canonical_text;
    ensure!(
        !normalized.ends_with("/*"),
        "directory exception must refer to a single directory"
    );
    let classification = repository_type(&canonical).await?;
    let root = match classification["kind"].as_str() {
        Some("unsafe") => classification["path"].as_str(),
        Some("regular") => classification["topLevelWorkingDirectory"].as_str(),
        _ => None,
    }
    .context("directory exception requires a repository root")?;
    ensure!(
        Path::new(root) == canonical,
        "directory exception requires the repository root"
    );
    #[cfg(windows)]
    let value = if normalized.starts_with('/') {
        format!("%(prefix)/{normalized}")
    } else {
        normalized
    };
    #[cfg(not(windows))]
    let value = canonical_text.to_owned();
    // Serialize this process's check and add; Git itself locks the config file
    // for writes. Retrying after cancellation is safe if the entry exists.
    static TRUST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = crate::cancellation::wait(TRUST_LOCK.lock()).await?;
    let cwd = std::env::temp_dir();
    let check = crate::process::run(
        &cwd,
        &crate::args(&[
            "config",
            "--global",
            "-z",
            "--fixed-value",
            "--get-all",
            "safe.directory",
            &value,
        ]),
    )
    .await?;
    ensure!(
        matches!(check.code, 0 | 1),
        "cannot read Git directory exceptions"
    );
    if !check
        .stdout
        .split(|byte| *byte == 0)
        .any(|entry| entry == value.as_bytes())
    {
        let added = crate::process::run(
            &cwd,
            &crate::args(&["config", "--global", "--add", "safe.directory", &value]),
        )
        .await?;
        ensure!(added.code == 0, "cannot add Git directory exception");
    }
    Ok(json!({"trusted":true}))
}
