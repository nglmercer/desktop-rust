//! Open a saved repository directory with the host file manager.
use crate::{Registry, Repository, cancellation};
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::path::PathBuf;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use {
    serde_json::json,
    std::{process::Stdio, time::Duration},
    tokio::sync::Semaphore,
};

#[cfg(any(target_os = "linux", target_os = "macos"))]
static OPENERS: Semaphore = Semaphore::const_new(8);

/// Reveal only a registered repository root using a fixed platform command.
pub async fn reveal_registered_repository(registry: Registry, path: PathBuf) -> Result<Value> {
    ensure!(path.is_absolute(), "repository path must be absolute");
    let canonical =
        cancellation::blocking(move || path.canonicalize().context("resolve repository root"))
            .await?;
    let lookup = canonical.clone();
    let saved = cancellation::blocking(move || registry.record_for_path(&lookup)).await?;
    ensure!(
        saved.is_some(),
        "Repository is not saved; add it before opening its folder"
    );
    let repository = Repository::open(&canonical).await?;
    ensure!(
        repository.path() == canonical,
        "select a repository root, not a nested path"
    );
    let target = canonical.clone();
    cancellation::blocking(move || {
        ensure!(
            target.canonicalize()? == target && target.is_dir(),
            "Repository path changed before opening"
        );
        Ok(())
    })
    .await?;
    launch(canonical).await
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
async fn launch(path: PathBuf) -> Result<Value> {
    let permit = OPENERS
        .try_acquire()
        .context("Too many file manager launches are still running")?;
    #[cfg(target_os = "macos")]
    let mut command = {
        // Reveal the directory rather than opening it, even if the target is
        // an application bundle or another executable package.
        let mut command = tokio::process::Command::new("/usr/bin/open");
        command.arg("-R").arg(&path);
        command
    };
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut command = tokio::process::Command::new("/usr/bin/xdg-open");
        command.arg(&path);
        // xdg-open is usually a shell script that resolves helper programs
        // through PATH. Do not let a renderer's checkout or inherited PATH
        // select those programs, and do not fall back to $BROWSER.
        command.env("PATH", "/usr/bin:/bin").env_remove("BROWSER");
        command
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    command.process_group(0);
    cancellation::Cancellation::current().check()?;
    let mut child = command
        .spawn()
        .context("Unable to start the file manager")?;
    // Keep the permit and reap the child even if the caller disconnects while
    // launch is being confirmed. Retrying after cancellation is uncertain.
    let mut monitor = tokio::spawn(async move {
        let _permit = permit;
        child.wait().await
    });
    match tokio::time::timeout(Duration::from_millis(200), &mut monitor).await {
        Ok(Ok(Ok(status))) if !status.success() => {
            bail!("The file manager could not open the repository")
        }
        Ok(Ok(Err(error))) => {
            return Err(error).context("Unable to confirm the file manager launch");
        }
        Ok(Err(error)) => return Err(error).context("Unable to monitor the file manager launch"),
        Ok(Ok(Ok(_))) => return Ok(json!({"launched": true})),
        Err(_) => {}
    }
    Ok(json!({"launched": true}))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
async fn launch(_path: PathBuf) -> Result<Value> {
    bail!("Opening a repository in the file manager is not supported on this platform")
}
