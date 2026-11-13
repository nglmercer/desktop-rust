//! Discover fetch remotes without returning credential-bearing URLs.
use crate::{Repository, args, process, validate_remote};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};

impl Repository {
    pub(crate) async fn safe_fetch_remotes(&self) -> Result<Value> {
        safe_fetch_remotes(&self.path).await
    }

    /// Apply a remote change without returning URLs or Git diagnostics.
    pub(crate) async fn manage_remote(&self, action: ManagedRemoteAction) -> Result<Value> {
        if !action.is_supported() {
            return Ok(json!({"kind":"unsupported"}));
        }
        let command = match &action {
            ManagedRemoteAction::Add { name, url } => args(&["remote", "add", "--", name, url]),
            ManagedRemoteAction::SetUrl { name, url } => {
                args(&["remote", "set-url", "--", name, url])
            }
            ManagedRemoteAction::Remove { name } => args(&["remote", "remove", "--", name]),
        };
        let output = process::run_remote(&self.path, &command).await?;
        if output.code != 0
            && !(matches!(action, ManagedRemoteAction::Remove { .. }) && output.code == 2)
        {
            bail!("Git remote update failed; inspect the repository configuration before retrying");
        }
        Ok(json!({"kind":"updated"}))
    }
}

#[derive(Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManagedRemoteAction {
    Add { name: String, url: String },
    Remove { name: String },
    SetUrl { name: String, url: String },
}

impl ManagedRemoteAction {
    pub(crate) fn is_supported(&self) -> bool {
        let (name, url) = match self {
            Self::Add { name, url } | Self::SetUrl { name, url } => (name, Some(url)),
            Self::Remove { name } => (name, None),
        };
        validate_remote(name).is_ok() && url.is_none_or(|value| safe_url(value))
    }
}

impl std::fmt::Debug for ManagedRemoteAction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Add { name, .. } => formatter
                .debug_struct("Add")
                .field("name", name)
                .field("url", &"<redacted>")
                .finish(),
            Self::SetUrl { name, .. } => formatter
                .debug_struct("SetUrl")
                .field("name", name)
                .field("url", &"<redacted>")
                .finish(),
            Self::Remove { name } => formatter
                .debug_struct("Remove")
                .field("name", name)
                .finish(),
        }
    }
}

/// Discover fetch remotes from an explicit directory, including bare repos.
pub async fn safe_fetch_remotes(path: &Path) -> Result<Value> {
    if !is_repository(path).await? {
        return Ok(json!([]));
    }
    let output = process::run(
        path,
        &args(&["config", "--null", "--get-regexp", "^remote\\..+\\.url$"]),
    )
    .await?;
    if !matches!(output.code, 0 | 1) {
        bail!("Unable to inspect Git remote configuration");
    }
    let Some(mut names) = remote_names(&output.stdout)? else {
        return Ok(Value::Null);
    };
    names.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    let mut remotes = Vec::with_capacity(names.len());
    for name in names {
        let result = process::run(
            path,
            &args(&[
                "-c",
                "transfer.credentialsInUrl=allow",
                "ls-remote",
                "--get-url",
                "--",
                &name,
            ]),
        )
        .await?;
        if result.code != 0 {
            bail!("Unable to resolve Git remote URL");
        }
        let Some(bytes) = result.stdout.strip_suffix(b"\n") else {
            bail!("Invalid Git remote response");
        };
        let Ok(url) = std::str::from_utf8(bytes) else {
            return Ok(Value::Null);
        };
        if !safe_url(url) {
            return Ok(Value::Null);
        }
        remotes.push(json!({"name":name,"url":url}));
    }
    let result = json!(remotes);
    if serde_json::to_vec(&result)?.len() > 60 * 1024 {
        return Ok(Value::Null);
    }
    Ok(result)
}

async fn is_repository(path: &Path) -> Result<bool> {
    anyhow::ensure!(path.is_absolute(), "repository path must be absolute");
    let discovery = process::repository_discovery(path, &args(&["rev-parse", "--git-dir"])).await?;
    if discovery.code != 0 {
        if discovery.code == 128
            && discovery
                .stderr
                .split(|byte| *byte == b'\n')
                .any(|line| line.starts_with(b"fatal: not a git repository"))
        {
            return Ok(false);
        }
        bail!("Unable to inspect Git repository");
    }
    Ok(true)
}

/// Resolve a named remote without returning credentials or ambiguous URLs.
pub async fn safe_remote_url(path: &Path, name: &str) -> Result<Value> {
    anyhow::ensure!(
        name.len() <= 4096 && !name.contains('\0'),
        "Invalid remote name"
    );
    if !is_repository(path).await? {
        return Ok(json!({"kind":"missing"}));
    }
    let output = process::run(path, &args(&["remote", "get-url", "--", name])).await?;
    if output.code == 2 {
        return Ok(json!({"kind":"missing"}));
    }
    if output.code != 0 {
        bail!("Unable to resolve Git remote URL");
    }
    let bytes = output
        .stdout
        .strip_suffix(b"\n")
        .context("Invalid Git remote response")?;
    let Ok(url) = std::str::from_utf8(bytes) else {
        return Ok(json!({"kind":"unsupported"}));
    };
    if !safe_url(url) {
        return Ok(json!({"kind":"unsupported"}));
    }
    Ok(json!({"kind":"resolved","url":url}))
}

fn remote_names(data: &[u8]) -> Result<Option<Vec<String>>> {
    if data.is_empty() {
        return Ok(Some(Vec::new()));
    }
    anyhow::ensure!(
        data.last() == Some(&0),
        "Incomplete Git remote configuration"
    );
    let mut names = BTreeSet::new();
    for entry in data
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let separator = entry
            .iter()
            .position(|byte| *byte == b'\n')
            .context("Remote URL configuration is missing a value")?;
        let key = &entry[..separator];
        let name = key
            .strip_prefix(b"remote.")
            .and_then(|key| key.strip_suffix(b".url"))
            .context("Invalid Git remote configuration")?;
        anyhow::ensure!(!name.is_empty(), "Invalid Git remote name");
        if entry[separator + 1..].is_empty() {
            names.remove(name);
        } else {
            names.insert(name);
        }
    }
    if names.len() > 1000 {
        return Ok(None);
    }
    Ok(names
        .into_iter()
        .map(|name| std::str::from_utf8(name).ok().map(str::to_owned))
        .collect())
}

/// Preserve the exact URL for editable settings. Ambiguous URL forms stay on
/// the old path rather than exposing a secret or silently changing the URL.
pub(crate) fn safe_url(value: &str) -> bool {
    if value.is_empty() || value.len() > 16384 || value.chars().any(char::is_control) {
        return false;
    }
    if let Some(rest) = value.strip_prefix("git@") {
        let Some((host, path)) = rest.split_once(':') else {
            return false;
        };
        return !host.is_empty()
            && host
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b".-".contains(&c))
            && !path.is_empty()
            && !path.contains(['?', '#']);
    }
    if value.starts_with('/') || value.starts_with("./") || value.starts_with("../") {
        return true;
    }
    // On Windows Git accepts drive-absolute paths as local remotes. Do not
    // admit drive-relative paths (such as C:repo), which are ambiguous with
    // scp-style remote syntax.
    #[cfg(windows)]
    if matches!(value.as_bytes(), [drive, b':', b'/' | b'\\', ..] if drive.is_ascii_alphabetic()) {
        return !value.contains('?');
    }
    // Git also accepts scp-style SSH remotes without an explicit username.
    // Keep usernames restricted to the known `git@` spelling above: an
    // arbitrary username can itself contain a credential. Reject `ext::` and
    // other remote-helper syntax, and avoid ambiguous drive-relative paths.
    if !value.contains("://") {
        if let Some((host, path)) = value.split_once(':') {
            if host.len() > 1
                && host.as_bytes()[0].is_ascii_alphanumeric()
                && host
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b".-".contains(&c))
                && !path.is_empty()
                && !path.starts_with(':')
                && !path.contains(['?', '#'])
            {
                return true;
            }
        }
    }
    if !value.contains([':', '@', '?', '#']) {
        return true;
    }
    let Ok(parsed) = url::Url::parse(value) else {
        return false;
    };
    if !matches!(parsed.scheme(), "http" | "https" | "ssh" | "git" | "file")
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !(parsed.username().is_empty()
            || (parsed.scheme() == "ssh" && parsed.username() == "git"))
    {
        return false;
    }
    parsed.scheme() == "file" || parsed.host_str().is_some()
}

#[cfg(test)]
mod tests {
    use super::{ManagedRemoteAction, remote_names, safe_url};

    #[test]
    fn debug_output_does_not_include_remote_credentials() {
        let action = ManagedRemoteAction::SetUrl {
            name: "origin".into(),
            url: "https://user:synthetic-secret@example.invalid/repo".into(),
        };
        let debug = format!("{action:?}");
        assert!(!debug.contains("synthetic-secret"));
        assert!(debug.contains("<redacted>"));
    }

    #[tokio::test]
    async fn unsupported_url_is_rejected_before_account_or_git_mutation() {
        let root =
            std::env::temp_dir().join(format!("desktop-remote-preflight-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(&root)
                .status()
                .unwrap()
                .success()
        );
        let registry = root.join("invalid-registry.json");
        std::fs::write(&registry, "not valid json").unwrap();
        let repository = crate::Repository::open(&root)
            .await
            .unwrap()
            .with_registry(crate::Registry::new(registry));
        let value = repository
            .execute(crate::Operation::ManageRemote {
                action: ManagedRemoteAction::Add {
                    name: "origin".into(),
                    url: "https://user:synthetic-secret@example.invalid/repo".into(),
                },
            })
            .await
            .unwrap();
        assert_eq!(value, serde_json::json!({"kind":"unsupported"}));
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["config", "--local", "--get", "remote.origin.url"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn discovers_and_manages_username_free_scp_remotes() {
        let root =
            std::env::temp_dir().join(format!("desktop-scp-remote-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(&root)
                .status()
                .unwrap()
                .success()
        );
        let repository = crate::Repository::open(&root).await.unwrap();
        let url = "desktop-scp-test.invalid:team/repo.git";
        assert_eq!(
            repository
                .execute(crate::Operation::ManageRemote {
                    action: ManagedRemoteAction::Add {
                        name: "origin".into(),
                        url: url.into(),
                    },
                })
                .await
                .unwrap(),
            serde_json::json!({"kind":"updated"})
        );
        assert_eq!(
            super::safe_fetch_remotes(&root).await.unwrap(),
            serde_json::json!([{"name":"origin", "url":url}])
        );
        assert_eq!(
            super::safe_remote_url(&root, "origin").await.unwrap(),
            serde_json::json!({"kind":"resolved", "url":url})
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reviewed_fetch_accepts_username_free_scp_remote() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("desktop-scp-fetch-{}", uuid::Uuid::new_v4()));
        let source = root.join("source");
        let target = root.join("target");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir(&target).unwrap();
        let git = |dir: &std::path::Path, args: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        git(&source, &["init", "-q", "-b", "main"]);
        git(
            &source,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ],
        );
        git(&target, &["init", "-q"]);
        let ssh = root.join("ssh-fixture");
        std::fs::write(&ssh, "#!/bin/sh\nshift\nexec sh -c \"$1\"\n").unwrap();
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).unwrap();
        git(
            &target,
            &["config", "core.sshCommand", ssh.to_str().unwrap()],
        );
        git(&target, &["config", "ssh.variant", "simple"]);
        let url = format!("desktop-scp-test.invalid:{}", source.display());
        git(&target, &["remote", "add", "origin", &url]);
        let repository = crate::Repository::open(&target).await.unwrap();
        let request: crate::RendererFetchRequest = serde_json::from_value(serde_json::json!({
            "remote":"origin", "expected_url":url, "recurse_submodules":false,
        }))
        .unwrap();
        let preview = repository
            .execute(crate::Operation::RendererFetchPreview {
                request: request.clone(),
            })
            .await
            .unwrap();
        let result = repository
            .execute(crate::Operation::RendererFetch {
                request,
                basis: preview["basis"].as_str().unwrap().to_owned(),
            })
            .await
            .unwrap();
        assert_eq!(result["code"], 0);
        assert_eq!(
            git(&target, &["rev-parse", "refs/remotes/origin/main"]),
            git(&source, &["rev-parse", "HEAD"])
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tracks_empty_overrides_without_returning_urls() {
        assert_eq!(remote_names(b"").unwrap().unwrap(), Vec::<String>::new());
        assert_eq!(
            remote_names(b"remote.origin.url\nvalue\0remote.origin.url\n\0")
                .unwrap()
                .unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            remote_names(b"remote.one.url\nvalue\0remote.two.url\nvalue\0remote.one.url\n\0")
                .unwrap()
                .unwrap(),
            vec!["two"]
        );
        assert!(remote_names(b"remote.origin.url\0").is_err());
        assert!(remote_names(b"remote.origin.url\nvalue").is_err());
    }

    #[test]
    fn rejects_ambiguous_and_credential_bearing_urls() {
        for value in [
            "https://github.com/owner/repo.git",
            "git@github.com:owner/repo.git",
            "github.com:owner/repo.git",
            "ssh-host.example:/srv/repo.git",
            "ssh://git@github.com/owner/repo",
            "/tmp/repo",
        ] {
            assert!(safe_url(value), "{value}");
        }
        for value in [
            "https://user:secret@host/repo",
            "https://host/repo?token=secret",
            "https://host/repo#secret",
            "ssh://token@host/repo",
            "token@host:repo",
            "ext::command",
            "-host:repo",
            "host:repo?token=secret",
            "C:project",
            "https://host/repo\nsecret",
        ] {
            assert!(!safe_url(value), "{value}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn accepts_drive_absolute_paths_but_not_drive_relative_paths() {
        assert!(safe_url(r"C:\repositories\project.git"));
        assert!(safe_url("d:/repositories/project.git"));
        assert!(safe_url(r"C:\repositories\project#1.git"));
        assert!(!safe_url("C:project"));
        assert!(!safe_url(r"C:\project?token=secret"));
    }
}
