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
