//! Durable copies of split-index dependencies for index recovery.
use crate::{Repository, args, cancellation, discard_fs as work, process};
use anyhow::{Context, Result, bail};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SharedIndex {
    pub(crate) name: String,
    common: bool,
    pub(crate) size: u64,
    pub(crate) sha256: String,
}

pub(crate) fn validate_name(name: &str) -> Result<()> {
    let hash = name
        .strip_prefix("sharedindex.")
        .context("invalid shared-index name")?;
    anyhow::ensure!(
        matches!(hash.len(), 40 | 64) && hash.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid shared-index name"
    );
    Ok(())
}

fn checksum(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(crate) fn read_shared_index(path: &Path) -> Result<Option<Vec<u8>>> {
    let parent = Dir::open_ambient_dir(
        path.parent().context("shared index has no parent")?,
        ambient_authority(),
    )?;
    let name = path.file_name().context("shared index has no name")?;
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = match parent.open_with(name, &options) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "shared index must be a regular file"
    );
    let mut bytes = Vec::new();
    file.take(64 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= 64 * 1024 * 1024,
        "shared index exceeds supported undo size"
    );
    Ok(Some(bytes))
}
fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn publish_shared_index(target: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = target.with_extension(format!("desktop-{}", uuid::Uuid::new_v4()));
    work::private_write(&temporary, bytes)?;
    let _cleanup = Temporary(temporary.clone());
    match fs::hard_link(&temporary, target) {
        Ok(()) => sync_directory(target.parent().context("shared index has no parent")?)?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            anyhow::ensure!(
                read_shared_index(target)?.as_deref() == Some(bytes),
                "shared index changed; undo data retained"
            );
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

impl Repository {
    pub(crate) async fn save_shared_indexes(
        &self,
        directory: &Path,
        original: &Option<Vec<u8>>,
        replacement: Option<&Vec<u8>>,
    ) -> Result<Vec<SharedIndex>> {
        let mut shared = Vec::<SharedIndex>::new();
        let git_directory = self.git_directory().await?.canonicalize()?;
        let common = self.common_directory().await?;
        for index in original.iter().chain(replacement) {
            let probe = directory.join(format!("index-{}", uuid::Uuid::new_v4()));
            work::private_write(&probe, index)?;
            let _cleanup = Temporary(probe.clone());
            let result = process::run_git(
                &self.path,
                &args(&["rev-parse", "--path-format=absolute", "--shared-index-path"]),
                None,
                Some(&probe),
            )
            .await?;
            if result.code != 0 {
                bail!(
                    "cannot inspect shared index: {}",
                    String::from_utf8_lossy(&result.stderr)
                );
            }
            let name = result.stdout.strip_suffix(b"\n").unwrap_or(&result.stdout);
            if name.is_empty() {
                continue;
            }
            let source = PathBuf::from(crate::os_from_bytes(name)?).canonicalize()?;
            let name = source
                .file_name()
                .and_then(|value| value.to_str())
                .context("invalid shared-index filename")?
                .to_owned();
            validate_name(&name)?;
            if shared.iter().any(|index| index.name == name) {
                continue;
            }
            let parent = source
                .parent()
                .context("shared index has no directory")?
                .to_owned();
            anyhow::ensure!(
                parent == git_directory || parent == common,
                "shared index is outside Git metadata"
            );
            let destination = directory.join(&name);
            let state = cancellation::blocking(move || {
                work::copy_state(fs::File::open(source)?, Some(&destination))
            })
            .await?;
            anyhow::ensure!(
                state.size <= 64 * 1024 * 1024,
                "shared index exceeds supported undo size"
            );
            shared.push(SharedIndex {
                name,
                common: parent == common && common != git_directory,
                size: state.size,
                sha256: state.sha256,
            });
        }
        Ok(shared)
    }

    pub(crate) async fn restore_shared_indexes(
        &self,
        directory: &Path,
        shared: &[SharedIndex],
    ) -> Result<()> {
        let git_directory = self.git_directory().await?.canonicalize()?;
        let common = self.common_directory().await?;
        for index in shared {
            validate_name(&index.name)?;
            let bytes = read_shared_index(&directory.join(&index.name))?
                .context("missing saved shared index")?;
            anyhow::ensure!(
                bytes.len() as u64 == index.size && checksum(&bytes) == index.sha256,
                "saved shared index was changed; undo data retained"
            );
            let target = if index.common {
                &common
            } else {
                &git_directory
            }
            .join(&index.name);
            match read_shared_index(&target)? {
                Some(existing) => anyhow::ensure!(
                    existing == bytes,
                    "shared index changed; undo data retained"
                ),
                None => publish_shared_index(&target, &bytes)?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn preserves_and_recovers_split_index_without_overwriting_conflicts() {
        let root =
            std::env::temp_dir().join(format!("desktop-shared-index-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        fs::write(root.join("file"), b"staged\n").unwrap();
        git(&["add", "file"]);
        git(&["update-index", "--split-index"]);
        let original = fs::read(root.join(".git/index")).unwrap();
        let backup = root.join("backup");
        fs::create_dir(&backup).unwrap();
        let repo = Repository::open(&root).await.unwrap();
        let saved = repo
            .save_shared_indexes(&backup, &Some(original), None)
            .await
            .unwrap();
        assert_eq!(saved.len(), 1);
        let target = root.join(".git").join(&saved[0].name);
        let expected = fs::read(&target).unwrap();
        fs::remove_file(&target).unwrap();
        fs::write(&target, b"concurrent modification").unwrap();
        assert!(repo.restore_shared_indexes(&backup, &saved).await.is_err());
        assert!(publish_shared_index(&target, &expected).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"concurrent modification");
        fs::remove_file(&target).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(backup.join(&saved[0].name), &target).unwrap();
            assert!(repo.restore_shared_indexes(&backup, &saved).await.is_err());
            assert!(
                fs::symlink_metadata(&target)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            fs::remove_file(&target).unwrap();
        }
        repo.restore_shared_indexes(&backup, &saved).await.unwrap();
        assert_eq!(fs::read(&target).unwrap(), expected);
        publish_shared_index(&target, &expected).unwrap();
        repo.restore_shared_indexes(&backup, &saved).await.unwrap();
        fs::write(backup.join(&saved[0].name), b"corrupt backup").unwrap();
        assert!(repo.restore_shared_indexes(&backup, &saved).await.is_err());
        assert_eq!(fs::read(&target).unwrap(), expected);
        fs::remove_dir_all(root).unwrap();
    }
}
