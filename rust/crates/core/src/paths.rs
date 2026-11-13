//! Native directory paths in persistent metadata and RPCs, preserving Unix bytes.
use crate::GitPath;
use anyhow::{Result, bail};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::{Path, PathBuf};

/// Check a directory without requiring a selected repository.
pub async fn directory_exists(path: &Path) -> Result<bool> {
    if !path.is_absolute() {
        bail!("directory path must be absolute");
    }
    Ok(tokio::fs::metadata(path)
        .await
        .is_ok_and(|entry| entry.is_dir()))
}

/// Check whether a path is visible to the process, following symlinks.
pub async fn path_exists(path: &Path) -> Result<bool> {
    if !path.is_absolute() {
        bail!("path must be absolute");
    }
    Ok(tokio::fs::metadata(path).await.is_ok())
}

/// Describe a prospective clone destination for UI validation. This is only
/// a snapshot: Git must still handle changes made after the check.
pub async fn clone_destination_status(path: &Path) -> Result<&'static str> {
    if !path.is_absolute() {
        bail!("clone destination must be absolute");
    }
    match tokio::fs::read_dir(path).await {
        Ok(mut directory) => match directory.next_entry().await {
            Ok(Some(_)) => Ok("not_empty"),
            Ok(None) => Ok("empty"),
            Err(_) => Ok("unreadable"),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Git can clone through a link to an existing directory, but a
            // dangling link at the destination or any ancestor blocks creation.
            for candidate in path.ancestors() {
                match tokio::fs::symlink_metadata(candidate).await {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        return match tokio::fs::metadata(candidate).await {
                            Ok(_) => Ok("missing"),
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                                Ok("dangling_symlink")
                            }
                            Err(_) => Ok("unreadable"),
                        };
                    }
                    Ok(_) => return Ok("missing"),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(_) => return Ok("unreadable"),
                }
            }
            Ok("missing")
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotADirectory => Ok("not_directory"),
        Err(_) => Ok("unreadable"),
    }
}

/// Resolve an existing path, or its nearest existing parent for a new target.
pub async fn canonical_mutation_path(path: &Path) -> Result<String> {
    if !path.is_absolute() {
        bail!("path must be absolute");
    }
    let mut current = path.to_path_buf();
    let mut missing = Vec::new();
    loop {
        match tokio::fs::canonicalize(&current).await {
            Ok(resolved) => {
                let mut resolved = crate::repository_discovery::display_canonical(resolved);
                for segment in missing.iter().rev() {
                    resolved.push(segment);
                }
                return crate::repository_discovery::normalize(&resolved)
                    .into_os_string()
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("resolved path is not UTF-8"));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let normalized = crate::repository_discovery::normalize(&current);
                let raw = current
                    .to_str()
                    .ok_or_else(|| anyhow::anyhow!("path is not UTF-8"))?;
                let separator =
                    |character| character == '/' || (cfg!(windows) && character == '\\');
                let name = raw
                    .trim_end_matches(separator)
                    .rsplit(separator)
                    .next()
                    .filter(|name| !name.is_empty())
                    .ok_or(error)?;
                missing.push(std::ffi::OsString::from(name));
                current = normalized
                    .parent()
                    .ok_or_else(|| anyhow::anyhow!("path has no existing parent"))?
                    .to_path_buf();
            }
            Err(error) => return Err(error.into()),
        }
    }
}

pub fn directory_path(path: GitPath) -> Result<PathBuf> {
    let value = match path {
        GitPath::Text(value) => value.into(),
        GitPath::Bytes { bytes } => crate::os_from_bytes(&bytes)?,
    };
    let path = PathBuf::from(value);
    if path.as_os_str().is_empty() || path.as_os_str().as_encoded_bytes().contains(&0) {
        bail!("invalid directory path");
    }
    Ok(path)
}
#[derive(Clone)]
struct SavedPath(PathBuf);
impl Serialize for SavedPath {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if let Some(value) = self.0.to_str() {
            serializer.serialize_str(value)
        } else {
            GitPath::Bytes {
                bytes: self.0.as_os_str().as_encoded_bytes().to_vec(),
            }
            .serialize(serializer)
        }
    }
}
impl<'de> Deserialize<'de> for SavedPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        directory_path(GitPath::deserialize(deserializer)?)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}
pub(crate) mod list {
    use super::*;
    pub fn serialize<S: Serializer>(value: &[PathBuf], s: S) -> Result<S::Ok, S::Error> {
        value
            .iter()
            .cloned()
            .map(SavedPath)
            .collect::<Vec<_>>()
            .serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<PathBuf>, D::Error> {
        Ok(Vec::<SavedPath>::deserialize(d)?
            .into_iter()
            .map(|value| value.0)
            .collect())
    }
}
pub(crate) mod optional {
    use super::*;
    pub fn serialize<S: Serializer>(value: &Option<PathBuf>, s: S) -> Result<S::Ok, S::Error> {
        value.clone().map(SavedPath).serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<PathBuf>, D::Error> {
        Ok(Option::<SavedPath>::deserialize(d)?.map(|value| value.0))
    }
}

#[cfg(test)]
mod clone_destination_tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn matches_git_for_dangling_and_valid_symlinks() {
        use std::process::{Command, Stdio};
        let root = std::env::temp_dir().join(format!("desktop-clone-git-{}", uuid::Uuid::new_v4()));
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        std::fs::create_dir(&root).unwrap();
        let _cleanup = Cleanup(root.clone());
        let run = |args: &[&std::ffi::OsStr]| {
            Command::new("git")
                .args(args)
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                .env("HOME", &root)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", root.join("no-config"))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
        };
        let remote = root.join("remote.git");
        assert!(run(&["init".as_ref(), "--bare".as_ref(), remote.as_os_str()]).success());
        let empty = root.join("empty");
        std::fs::create_dir(&empty).unwrap();
        let valid = root.join("valid");
        let dangling = root.join("dangling");
        std::os::unix::fs::symlink(&empty, &valid).unwrap();
        std::os::unix::fs::symlink(root.join("absent"), &dangling).unwrap();
        assert_eq!(clone_destination_status(&valid).await.unwrap(), "empty");
        assert!(run(&["clone".as_ref(), remote.as_os_str(), valid.as_os_str()]).success());
        for destination in [dangling.clone(), dangling.join("child")] {
            assert_eq!(
                clone_destination_status(&destination).await.unwrap(),
                "dangling_symlink"
            );
            assert!(
                !run(&[
                    "clone".as_ref(),
                    remote.as_os_str(),
                    destination.as_os_str()
                ])
                .success()
            );
        }
    }

    #[tokio::test]
    async fn classifies_real_destinations_and_does_not_change_them() {
        let root =
            std::env::temp_dir().join(format!("desktop-clone-status-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir(&root).await.unwrap();
        let empty = root.join("empty");
        let file = root.join("file");
        tokio::fs::create_dir(&empty).await.unwrap();
        tokio::fs::write(&file, "original").await.unwrap();
        assert_eq!(
            clone_destination_status(&root.join("missing"))
                .await
                .unwrap(),
            "missing"
        );
        assert_eq!(clone_destination_status(&empty).await.unwrap(), "empty");
        assert_eq!(
            clone_destination_status(&file).await.unwrap(),
            "not_directory"
        );
        assert_eq!(
            clone_destination_status(&file.join("child")).await.unwrap(),
            "not_directory"
        );
        assert_eq!(clone_destination_status(&root).await.unwrap(), "not_empty");
        assert!(
            clone_destination_status(Path::new("relative"))
                .await
                .is_err()
        );
        assert_eq!(tokio::fs::read_to_string(&file).await.unwrap(), "original");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&empty, root.join("link-empty")).unwrap();
            std::os::unix::fs::symlink(&file, root.join("link-file")).unwrap();
            std::os::unix::fs::symlink(root.join("missing"), root.join("dangling")).unwrap();
            assert_eq!(
                clone_destination_status(&root.join("link-empty"))
                    .await
                    .unwrap(),
                "empty"
            );
            assert_eq!(
                clone_destination_status(&root.join("link-file"))
                    .await
                    .unwrap(),
                "not_directory"
            );
            assert_eq!(
                clone_destination_status(&root.join("dangling"))
                    .await
                    .unwrap(),
                "dangling_symlink"
            );
            assert_eq!(
                clone_destination_status(&root.join("dangling/child"))
                    .await
                    .unwrap(),
                "dangling_symlink"
            );
            assert_eq!(
                clone_destination_status(&root.join("link-empty/child"))
                    .await
                    .unwrap(),
                "missing"
            );
            use std::os::unix::fs::PermissionsExt;
            let private = root.join("private");
            tokio::fs::create_dir(&private).await.unwrap();
            tokio::fs::set_permissions(&private, std::fs::Permissions::from_mode(0))
                .await
                .unwrap();
            if tokio::fs::read_dir(&private).await.is_err() {
                assert_eq!(
                    clone_destination_status(&private).await.unwrap(),
                    "unreadable"
                );
            }
            tokio::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700))
                .await
                .unwrap();
        }
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
