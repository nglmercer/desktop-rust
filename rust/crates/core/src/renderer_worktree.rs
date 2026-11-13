//! Reviewed worktree operations for the desktop renderer.
use crate::{GitPath, Repository, args, process};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};

/// Force removal destroys ignored and untracked files too. Bind every entry.
pub(crate) fn removal_tree_certificate(path: &Path) -> Result<Vec<u8>> {
    tree_certificate(path, true)
}

/// Bind all operation state bytes while refusing links which Git could follow.
pub(crate) fn operation_tree_certificate(path: &Path) -> Result<Vec<u8>> {
    tree_certificate(path, false)
}

fn tree_certificate(path: &Path, allow_symlinks: bool) -> Result<Vec<u8>> {
    use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
    use cap_std::{
        ambient_authority,
        fs::{Dir, OpenOptions},
    };
    use std::io::Read;
    const MAX_ENTRIES: usize = 100_000;
    const MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
    fn walk(
        dir: &Dir,
        digest: &mut Sha256,
        entries: &mut usize,
        bytes: &mut u64,
        depth: usize,
        allow_symlinks: bool,
    ) -> Result<()> {
        anyhow::ensure!(
            depth <= 128,
            "Worktree nesting is too deep for safe force removal"
        );
        crate::cancellation::Cancellation::current().check()?;
        let before = dir.dir_metadata()?;
        #[cfg(unix)]
        {
            use cap_std::fs::MetadataExt;
            digest.update(before.dev().to_le_bytes());
            digest.update(before.ino().to_le_bytes());
            digest.update(before.mode().to_le_bytes());
        }
        let mut names = Vec::new();
        for entry in dir.entries()? {
            crate::cancellation::Cancellation::current().check()?;
            anyhow::ensure!(
                entries.saturating_add(names.len()) < MAX_ENTRIES,
                "Worktree contains too many entries for safe force removal"
            );
            names.push(entry?.file_name());
        }
        names.sort_by(|a, b| a.as_encoded_bytes().cmp(b.as_encoded_bytes()));
        for name in names {
            crate::cancellation::Cancellation::current().check()?;
            *entries += 1;
            anyhow::ensure!(
                *entries <= MAX_ENTRIES,
                "Worktree contains too many entries for safe force removal"
            );
            let name_bytes = name.as_encoded_bytes();
            digest.update((name_bytes.len() as u64).to_le_bytes());
            digest.update(name_bytes);
            let metadata = dir.symlink_metadata(&name)?;
            #[cfg(unix)]
            {
                use cap_std::fs::MetadataExt;
                digest.update(metadata.dev().to_le_bytes());
                digest.update(metadata.ino().to_le_bytes());
                digest.update(metadata.mode().to_le_bytes());
                digest.update(metadata.nlink().to_le_bytes());
                digest.update(metadata.uid().to_le_bytes());
                digest.update(metadata.gid().to_le_bytes());
                digest.update(metadata.mtime().to_le_bytes());
                digest.update(metadata.mtime_nsec().to_le_bytes());
                digest.update(metadata.ctime().to_le_bytes());
                digest.update(metadata.ctime_nsec().to_le_bytes());
            }
            if metadata.is_dir() {
                digest.update(b"directory\0");
                let child = dir.open_dir_nofollow(&name)?;
                walk(&child, digest, entries, bytes, depth + 1, allow_symlinks)?;
            } else if metadata.file_type().is_symlink() {
                anyhow::ensure!(
                    allow_symlinks,
                    "Git operation state contains a symbolic link"
                );
                digest.update(b"symlink\0");
                let target = dir.read_link_contents(&name)?;
                digest.update((target.as_os_str().as_encoded_bytes().len() as u64).to_le_bytes());
                digest.update(target.as_os_str().as_encoded_bytes());
            } else if metadata.is_file() {
                *bytes = bytes.saturating_add(metadata.len());
                anyhow::ensure!(
                    *bytes <= MAX_BYTES,
                    "Worktree exceeds the size supported for safe force removal"
                );
                digest.update(b"file\0");
                digest.update(metadata.len().to_le_bytes());
                let mut options = OpenOptions::new();
                options.read(true).follow(FollowSymlinks::No);
                #[cfg(unix)]
                {
                    use cap_std::fs::OpenOptionsExt;
                    options.custom_flags(libc::O_NONBLOCK);
                }
                let mut file = dir.open_with(&name, &options)?.into_std();
                let opened = file.metadata()?;
                anyhow::ensure!(
                    opened.is_file() && opened.len() == metadata.len(),
                    "Worktree file changed during review"
                );
                #[cfg(unix)]
                {
                    use cap_std::fs::MetadataExt as CapMetadataExt;
                    use std::os::unix::fs::MetadataExt as StdMetadataExt;
                    anyhow::ensure!(
                        CapMetadataExt::dev(&metadata) == StdMetadataExt::dev(&opened)
                            && CapMetadataExt::ino(&metadata) == StdMetadataExt::ino(&opened),
                        "Worktree file identity changed during review"
                    );
                    digest.update(StdMetadataExt::mode(&opened).to_le_bytes());
                }
                let mut read = 0_u64;
                let mut chunk = [0_u8; 64 * 1024];
                loop {
                    crate::cancellation::Cancellation::current().check()?;
                    let count = file.read(&mut chunk)?;
                    if count == 0 {
                        break;
                    }
                    read += count as u64;
                    anyhow::ensure!(read <= opened.len(), "Worktree file changed during review");
                    digest.update(&chunk[..count]);
                }
                let after = file.metadata()?;
                anyhow::ensure!(
                    read == opened.len()
                        && after.len() == opened.len()
                        && after.modified().ok() == opened.modified().ok(),
                    "Worktree file changed during review"
                );
                let current = dir.symlink_metadata(&name)?;
                anyhow::ensure!(
                    current.is_file()
                        && current.len() == metadata.len()
                        && current.modified().ok() == metadata.modified().ok(),
                    "Worktree file changed during review"
                );
                #[cfg(unix)]
                {
                    use cap_std::fs::MetadataExt as CapMetadataExt;
                    use std::os::unix::fs::MetadataExt as StdMetadataExt;
                    anyhow::ensure!(
                        CapMetadataExt::dev(&current) == StdMetadataExt::dev(&opened)
                            && CapMetadataExt::ino(&current) == StdMetadataExt::ino(&opened)
                            && StdMetadataExt::mode(&after) == StdMetadataExt::mode(&opened)
                            && CapMetadataExt::mode(&current) == StdMetadataExt::mode(&opened),
                        "Worktree file identity changed during review"
                    );
                }
            } else {
                anyhow::bail!("Worktree contains a special file that cannot be safely reviewed");
            }
        }
        let after = dir.dir_metadata()?;
        anyhow::ensure!(
            before.modified().ok() == after.modified().ok(),
            "Worktree directory changed during review"
        );
        #[cfg(unix)]
        {
            use cap_std::fs::MetadataExt;
            anyhow::ensure!(
                before.dev() == after.dev()
                    && before.ino() == after.ino()
                    && before.mode() == after.mode()
                    && before.ctime() == after.ctime()
                    && before.ctime_nsec() == after.ctime_nsec(),
                "Worktree directory changed during review"
            );
        }
        digest.update(b"end-directory\0");
        Ok(())
    }
    let parent = path.parent().context("Worktree has no parent directory")?;
    let name = path.file_name().context("Worktree has no directory name")?;
    let parent = Dir::open_ambient_dir(parent, ambient_authority())?;
    let root = parent.open_dir_nofollow(name)?;
    let mut digest = Sha256::new();
    walk(&root, &mut digest, &mut 0, &mut 0, 0, allow_symlinks)?;
    Ok(digest.finalize().to_vec())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorktreeAction {
    Add {
        path: GitPath,
        create_branch: Option<String>,
        commitish: Option<String>,
    },
    Remove {
        path: GitPath,
        force: bool,
    },
    Move {
        from: GitPath,
        to: GitPath,
    },
}

fn checked_path(path: GitPath) -> Result<PathBuf> {
    let path = crate::directory_path(path)?;
    anyhow::ensure!(
        path.is_absolute()
            && !path
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::CurDir)),
        "Worktree path must be absolute and normalized"
    );
    Ok(path)
}

/// Capture identity for each path component, including absent destinations.
fn path_certificate(path: &Path) -> Result<Vec<u8>> {
    let mut hash = Sha256::new();
    for component in path.ancestors() {
        hash.update(component.as_os_str().as_encoded_bytes());
        match std::fs::symlink_metadata(component) {
            Ok(metadata) => {
                anyhow::ensure!(
                    !metadata.file_type().is_symlink(),
                    "Worktree paths cannot traverse symlinks"
                );
                hash.update([1, u8::from(metadata.is_dir()), u8::from(metadata.is_file())]);
                // Unrelated siblings may change an ancestor directory's mtime.
                // Its stable identity, rather than its listing, binds traversal.
                if component == path {
                    hash.update(metadata.len().to_le_bytes());
                    if let Ok(time) = metadata.modified().and_then(|time| {
                        time.duration_since(std::time::UNIX_EPOCH)
                            .map_err(std::io::Error::other)
                    }) {
                        hash.update(time.as_nanos().to_le_bytes());
                    }
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    hash.update(metadata.dev().to_le_bytes());
                    hash.update(metadata.ino().to_le_bytes());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => hash.update([0]),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(hash.finalize().to_vec())
}

impl Repository {
    async fn renderer_worktree_basis(&self, action: &WorktreeAction) -> Result<String> {
        let list = self
            .run(&args(&["worktree", "list", "--porcelain", "-z"]))
            .await?;
        anyhow::ensure!(list.code == 0, "Cannot inspect registered worktrees");
        let mut hash = Sha256::new();
        hash.update(b"renderer-worktree-v1\0");
        hash.update(serde_json::to_vec(action)?);
        hash.update(&list.stdout);
        let refs = self.run(&args(&["show-ref", "--head"])).await?;
        anyhow::ensure!(
            matches!(refs.code, 0 | 1),
            "Cannot inspect worktree branch references"
        );
        hash.update(refs.stdout);
        let config = process::configuration(
            &self.path,
            &args(&["config", "--null", "--list", "--show-origin", "--includes"]),
        )
        .await?;
        anyhow::ensure!(config.code == 0, "Cannot inspect worktree configuration");
        hash.update(config.stdout);
        let (source, destination) = match action {
            WorktreeAction::Add {
                path,
                create_branch,
                commitish,
            } => {
                let path = checked_path(path.clone())?;
                if let Some(branch) = create_branch {
                    self.validate_branch(branch).await?;
                    self.branch_name_rules(branch).await?.ensure_allowed()?;
                } else {
                    // Git may create a branch implicitly from the directory
                    // name, or DWIM an unresolved name to a remote branch.
                    let candidate = match commitish {
                        None => path.file_name().and_then(|name| name.to_str()),
                        Some(revision) => Some(revision.as_str()),
                    };
                    if let Some(candidate) =
                        candidate.filter(|candidate| !candidate.starts_with('-'))
                    {
                        let check = self
                            .run(&args(&["check-ref-format", "--branch", candidate]))
                            .await?;
                        if check.code == 0 {
                            let local = self
                                .run(&args(&[
                                    "show-ref",
                                    "--verify",
                                    "--quiet",
                                    &format!("refs/heads/{candidate}"),
                                ]))
                                .await?;
                            anyhow::ensure!(
                                matches!(local.code, 0 | 1),
                                "Cannot inspect worktree branch"
                            );
                            let needs_policy = if local.code == 0 {
                                false
                            } else if commitish.is_none() {
                                true
                            } else {
                                let revision = format!("{candidate}^{{commit}}");
                                let resolved = self
                                    .run(&args(&[
                                        "rev-parse",
                                        "--verify",
                                        "--quiet",
                                        "--end-of-options",
                                        &revision,
                                    ]))
                                    .await?;
                                anyhow::ensure!(
                                    matches!(resolved.code, 0 | 1 | 128),
                                    "Cannot inspect worktree revision"
                                );
                                resolved.code != 0
                            };
                            if needs_policy {
                                self.branch_name_rules(candidate).await?.ensure_allowed()?;
                            }
                        }
                    }
                }
                if let Some(revision) = commitish {
                    anyhow::ensure!(
                        !revision.is_empty() && revision.len() <= 4096 && !revision.contains('\0'),
                        "Invalid worktree revision"
                    );
                }
                (None, Some(path))
            }
            WorktreeAction::Remove { path, .. } => {
                (Some(self.known_worktree(path.clone()).await?), None)
            }
            WorktreeAction::Move { from, to } => (
                Some(self.known_worktree(from.clone()).await?),
                Some(checked_path(to.clone())?),
            ),
        };
        if let Some(path) = &source {
            let records = self.worktree_records().await?;
            let entry = records
                .iter()
                .find(|entry| {
                    self.absolute_worktree_path(entry.path.clone())
                        .ok()
                        .as_ref()
                        == Some(path)
                })
                .context("Worktree is no longer registered")?;
            anyhow::ensure!(
                !entry.main && !entry.bare,
                "The main worktree cannot be changed here"
            );
            let source_certificate = path_certificate(path)?;
            hash.update(&source_certificate);
            if path.exists() {
                let opened = Repository::open(path)
                    .await
                    .context("Cannot open the registered worktree")?;
                anyhow::ensure!(
                    opened.path() == path.canonicalize()?
                        && opened.common_directory().await? == self.common_directory().await?,
                    "Worktree identity changed"
                );
                let status = process::run(
                    path,
                    &args(&["status", "--porcelain=v1", "-z", "--untracked-files=all"]),
                )
                .await?;
                anyhow::ensure!(status.code == 0, "Cannot inspect worktree changes");
                hash.update(status.stdout);
                if matches!(action, WorktreeAction::Remove { .. }) {
                    let path = path.clone();
                    hash.update(
                        crate::cancellation::blocking(move || removal_tree_certificate(&path))
                            .await?,
                    );
                }
                anyhow::ensure!(
                    path_certificate(path)? == source_certificate,
                    "Worktree path changed during review"
                );
            }
        }
        if let Some(path) = &destination {
            hash.update(path_certificate(path)?);
            if matches!(action, WorktreeAction::Move { .. })
                && let Some(registry) = self.account_registry()
            {
                anyhow::ensure!(
                    registry.record_for_path(path)?.is_none(),
                    "Destination already has a saved repository; review it before moving the worktree"
                );
            }
        }
        Ok(format!("{:x}", hash.finalize()))
    }

    pub(crate) async fn renderer_worktree_preview(&self, action: &WorktreeAction) -> Result<Value> {
        Ok(json!({"basis":self.renderer_worktree_basis(action).await?}))
    }

    pub(crate) async fn renderer_worktree_apply(
        &self,
        action: WorktreeAction,
        basis: &str,
    ) -> Result<Value> {
        let _lock = self.stash_lock().await?;
        anyhow::ensure!(
            self.renderer_worktree_basis(&action).await? == basis,
            "Worktree context changed; review it again"
        );
        let mut command = args(&["worktree"]);
        let mut cwd = self.path.clone();
        let mut relocated = None;
        let is_add = matches!(&action, WorktreeAction::Add { .. });
        match action {
            WorktreeAction::Add {
                path,
                create_branch,
                commitish,
            } => {
                command.push("add".into());
                if let Some(branch) = create_branch {
                    command.extend(args(&["-b", &branch]));
                }
                command.push("--".into());
                command.push(checked_path(path)?.into_os_string());
                if let Some(commitish) = commitish {
                    command.push(commitish.into());
                }
            }
            WorktreeAction::Remove { path, force } => {
                let path = self.known_worktree(path).await?;
                anyhow::ensure!(
                    path.canonicalize().unwrap_or(path.clone()) != self.path,
                    "Select another worktree before removing this one"
                );
                let records = self.worktree_records().await?;
                let entry = records
                    .iter()
                    .find(|entry| {
                        self.absolute_worktree_path(entry.path.clone())
                            .ok()
                            .as_ref()
                            == Some(&path)
                    })
                    .context("Worktree is no longer registered")?;
                if entry.locked.is_some() {
                    return Ok(json!({"kind":"refused", "message":"The worktree is locked"}));
                }
                if !force && path.exists() {
                    let status = process::run(
                        &path,
                        &args(&["status", "--porcelain=v1", "-z", "--untracked-files=all"]),
                    )
                    .await?;
                    anyhow::ensure!(status.code == 0, "Cannot inspect worktree changes");
                    if !status.stdout.is_empty() {
                        return Ok(
                            json!({"kind":"refused", "message":"The worktree contains modified or untracked files"}),
                        );
                    }
                }
                command.push("remove".into());
                if force {
                    command.push("--force".into());
                }
                command.push("--".into());
                command.push(path.into_os_string());
            }
            WorktreeAction::Move { from, to } => {
                let from = self.known_worktree(from).await?;
                let to = checked_path(to)?;
                let current = from.canonicalize().unwrap_or(from.clone()) == self.path;
                let source = if current {
                    self.saved_record()?
                } else if let Some(registry) = self.account_registry() {
                    registry.record_for_path(&from)?
                } else {
                    None
                };
                if current {
                    let main = self
                        .worktree_records()
                        .await?
                        .into_iter()
                        .find(|record| record.main)
                        .context("Cannot find main worktree")?;
                    cwd = self.absolute_worktree_path(main.path)?;
                }
                relocated = source.map(|source| (source, to.clone(), current));
                command.extend(args(&["move", "--"]));
                command.push(from.into_os_string());
                command.push(to.into_os_string());
            }
        }
        let result = process::run(&cwd, &command).await?;
        if result.code == 0
            && let Some((source, target, follow_current)) = relocated
            && let Some(registry) = self.account_registry()
        {
            let registry = registry.clone();
            let moved = target.clone();
            crate::cancellation::Cancellation::default().scope(crate::cancellation::blocking(move || registry.relocate_worktree_record(&source, moved, follow_current))).await
                .context("Worktree moved, but the saved repository could not be updated; inspect the new location before retrying")?;
        }
        if is_add {
            // Checkout filters and hooks may emit rewritten URLs or other
            // credentials unknown to the account broker.
            return Ok(
                json!({"code":result.code,"stdout":"","stderr":if result.code == 0 { "" } else { "Git could not finish adding the worktree" }}),
            );
        }
        Ok(serde_json::to_value(result.text())?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process::Command};
    #[tokio::test]
    async fn reviewed_worktree_lifecycle_checks_dirty_locks_and_moves_selected() {
        let root =
            std::env::temp_dir().join(format!("desktop-worktrees-review-{}", uuid::Uuid::new_v4()));
        let main = root.join("main");
        fs::create_dir_all(&main).unwrap();
        let git = |arguments: &[&str]| {
            let output = Command::new("git")
                .current_dir(&main)
                .args(arguments)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.invalid"]);
        git(&["config", "commit.gpgsign", "false"]);
        fs::write(main.join("file"), "base\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "base"]);
        let oid = git(&["rev-parse", "HEAD"]);
        let repo = Repository::open(&main).await.unwrap();
        let linked = root.join("linked");
        let add = WorktreeAction::Add {
            path: GitPath::Text(linked.to_str().unwrap().into()),
            create_branch: Some("topic".into()),
            commitish: Some(oid.clone()),
        };
        let basis = repo.renderer_worktree_basis(&add).await.unwrap();
        assert_eq!(
            repo.renderer_worktree_apply(add, &basis).await.unwrap()["code"],
            0
        );
        assert_eq!(
            git(&[
                "-C",
                linked.to_str().unwrap(),
                "symbolic-ref",
                "--short",
                "HEAD"
            ]),
            "topic"
        );
        let remove = WorktreeAction::Remove {
            path: GitPath::Text(linked.to_str().unwrap().into()),
            force: false,
        };
        let stale = repo.renderer_worktree_basis(&remove).await.unwrap();
        fs::write(linked.join("untracked"), "keep").unwrap();
        assert!(
            repo.renderer_worktree_apply(remove.clone(), &stale)
                .await
                .is_err()
        );
        let basis = repo.renderer_worktree_basis(&remove).await.unwrap();
        assert_eq!(
            repo.renderer_worktree_apply(remove, &basis).await.unwrap()["kind"],
            "refused"
        );
        assert_eq!(fs::read(linked.join("untracked")).unwrap(), b"keep");
        fs::write(linked.join(".gitignore"), "ignored\n").unwrap();
        fs::write(linked.join("ignored"), "old").unwrap();
        fs::write(linked.join("file"), "dirty").unwrap();
        let no_force = WorktreeAction::Remove {
            path: GitPath::Text(linked.to_str().unwrap().into()),
            force: false,
        };
        let stale = repo.renderer_worktree_basis(&no_force).await.unwrap();
        fs::write(linked.join("ignored"), "new").unwrap();
        assert!(
            repo.renderer_worktree_apply(no_force, &stale)
                .await
                .is_err()
        );
        fs::write(linked.join("ignored"), "old").unwrap();
        let force = WorktreeAction::Remove {
            path: GitPath::Text(linked.to_str().unwrap().into()),
            force: true,
        };
        let stale = repo.renderer_worktree_basis(&force).await.unwrap();
        fs::write(linked.join("ignored"), "new").unwrap();
        assert!(
            repo.renderer_worktree_apply(force.clone(), &stale)
                .await
                .is_err()
        );
        let stale = repo.renderer_worktree_basis(&force).await.unwrap();
        fs::write(linked.join("file"), "changed").unwrap();
        assert!(
            repo.renderer_worktree_apply(force.clone(), &stale)
                .await
                .is_err()
        );
        assert_eq!(fs::read(linked.join("ignored")).unwrap(), b"new");
        git(&["worktree", "lock", linked.to_str().unwrap()]);
        let basis = repo.renderer_worktree_basis(&force).await.unwrap();
        assert_eq!(
            repo.renderer_worktree_apply(force, &basis).await.unwrap()["kind"],
            "refused"
        );
        git(&["worktree", "unlock", linked.to_str().unwrap()]);
        let registry = crate::Registry::new(root.join("registry.json"));
        registry.add(linked.clone(), true).unwrap();
        let selected = Repository::open(&linked)
            .await
            .unwrap()
            .with_registry(registry.clone());
        let moved = root.join("moved");
        let action = WorktreeAction::Move {
            from: GitPath::Text(linked.to_str().unwrap().into()),
            to: GitPath::Text(moved.to_str().unwrap().into()),
        };
        let basis = selected.renderer_worktree_basis(&action).await.unwrap();
        assert_eq!(
            selected
                .renderer_worktree_apply(action, &basis)
                .await
                .unwrap()["code"],
            0
        );
        assert!(!linked.exists());
        assert_eq!(fs::read(moved.join("untracked")).unwrap(), b"keep");
        assert!(registry.record_for_path(&linked).unwrap().is_none());
        assert!(registry.record_for_path(&moved).unwrap().is_some());
        let second = root.join("second");
        let add = WorktreeAction::Add {
            path: GitPath::Text(second.to_str().unwrap().into()),
            create_branch: Some("second".into()),
            commitish: None,
        };
        let basis = repo.renderer_worktree_basis(&add).await.unwrap();
        assert_eq!(
            repo.renderer_worktree_apply(add, &basis).await.unwrap()["code"],
            0
        );
        registry.add(main.clone(), true).unwrap();
        registry.add(second.clone(), false).unwrap();
        let original_record = registry.record_for_path(&second).unwrap().unwrap();
        let other_repo = Repository::open(&main)
            .await
            .unwrap()
            .with_registry(registry.clone());
        let second_target = root.join("second-moved");
        let action = WorktreeAction::Move {
            from: GitPath::Text(second.to_str().unwrap().into()),
            to: GitPath::Text(second_target.to_str().unwrap().into()),
        };
        let basis = other_repo.renderer_worktree_basis(&action).await.unwrap();
        assert_eq!(
            other_repo
                .renderer_worktree_apply(action, &basis)
                .await
                .unwrap()["code"],
            0
        );
        assert_eq!(
            registry
                .record_for_path(&second_target)
                .unwrap()
                .unwrap()
                .id,
            original_record.id
        );
        assert_eq!(registry.read().unwrap().selected, Some(main.clone()));
        let detached = root.join("detached");
        let add = WorktreeAction::Add {
            path: GitPath::Text(detached.to_str().unwrap().into()),
            create_branch: None,
            commitish: Some(oid),
        };
        let basis = repo.renderer_worktree_basis(&add).await.unwrap();
        assert_eq!(
            repo.renderer_worktree_apply(add, &basis).await.unwrap()["code"],
            0
        );
        assert!(
            !Command::new("git")
                .current_dir(&detached)
                .args(["symbolic-ref", "--quiet", "HEAD"])
                .status()
                .unwrap()
                .success()
        );
        let default_path = root.join("automatic-branch");
        let add = WorktreeAction::Add {
            path: GitPath::Text(default_path.to_str().unwrap().into()),
            create_branch: None,
            commitish: None,
        };
        let basis = repo.renderer_worktree_basis(&add).await.unwrap();
        assert_eq!(
            repo.renderer_worktree_apply(add, &basis).await.unwrap()["code"],
            0
        );
        assert_eq!(
            git(&[
                "-C",
                default_path.to_str().unwrap(),
                "symbolic-ref",
                "--short",
                "HEAD"
            ]),
            "automatic-branch"
        );
        git(&["remote", "add", "origin", "/not-used"]);
        git(&[
            "update-ref",
            "refs/remotes/origin/remote-only",
            &git(&["rev-parse", "HEAD"]),
        ]);
        let tracked = root.join("tracked");
        let add = WorktreeAction::Add {
            path: GitPath::Text(tracked.to_str().unwrap().into()),
            create_branch: None,
            commitish: Some("remote-only".into()),
        };
        let basis = repo.renderer_worktree_basis(&add).await.unwrap();
        assert_eq!(
            repo.renderer_worktree_apply(add, &basis).await.unwrap()["code"],
            0
        );
        assert_eq!(
            git(&[
                "-C",
                tracked.to_str().unwrap(),
                "symbolic-ref",
                "--short",
                "HEAD"
            ]),
            "remote-only"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let hook = main.join(".git/hooks/post-checkout");
            fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
            let partial = root.join("partial");
            let add = WorktreeAction::Add {
                path: GitPath::Text(partial.to_str().unwrap().into()),
                create_branch: Some("partial".into()),
                commitish: None,
            };
            let basis = repo.renderer_worktree_basis(&add).await.unwrap();
            assert_ne!(
                repo.renderer_worktree_apply(add, &basis).await.unwrap()["code"],
                0
            );
            assert!(
                partial.exists(),
                "Git may leave a worktree after a failing hook"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }
}
