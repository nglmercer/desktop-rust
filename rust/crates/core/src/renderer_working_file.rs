//! Bounded, literal worktree file snapshots for renderer consumers.
use crate::{GitPath, Repository};
use anyhow::{Context, Result};
use cap_std::{ambient_authority, fs::Dir};
use std::path::Path;

impl Repository {
    pub(crate) async fn renderer_working_file(
        &self,
        path: GitPath,
        max_bytes: Option<u64>,
    ) -> Result<serde_json::Value> {
        let limit = max_bytes.unwrap_or(crate::comparison_snapshots::MAX_BYTES as u64);
        anyhow::ensure!(
            limit <= crate::comparison_snapshots::MAX_BYTES as u64,
            "Working file limit exceeds the supported size"
        );
        let path = crate::discard_fs::checked_path(path)?;
        let permit = crate::comparison_snapshots::reserve().await?;
        let root = self.path.clone();
        let bytes = crate::cancellation::blocking(move || {
            let root = Dir::open_ambient_dir(&root, ambient_authority())?;
            // Resolve links only within the capability root, then open the
            // resolved components without following any newly swapped link.
            let resolved = root
                .canonicalize(Path::new(&path))
                .context("Cannot resolve working image inside the repository")?;
            for component in resolved.components() {
                let std::path::Component::Normal(name) = component else {
                    anyhow::bail!("Invalid working image path");
                };
                let name = name.to_string_lossy();
                anyhow::ensure!(
                    !name
                        .trim_end_matches(['.', ' '])
                        .eq_ignore_ascii_case(".git")
                        && !(cfg!(windows) && name.contains(':')),
                    "Git metadata cannot be previewed as a working image"
                );
            }
            let file = crate::discard_fs::WorkingFile::open(&root, &resolved, false)?
                .context("Working file does not exist")?;
            let bytes = match file.read_bounded(limit) {
                Ok(bytes) => bytes,
                Err(error)
                    if max_bytes.is_some()
                        && error.is::<crate::discard_fs::WorkingFileTooLarge>() =>
                {
                    return Ok(None);
                }
                Err(error) => return Err(error),
            };
            anyhow::ensure!(
                root.canonicalize(Path::new(&path))
                    .is_ok_and(|current| current == resolved),
                "Working image path changed while reading"
            );
            Ok(Some(bytes))
        })
        .await?;
        let Some(bytes) = bytes else {
            return Ok(serde_json::json!({"kind":"too_large"}));
        };
        let cancellation = crate::cancellation::Cancellation::current();
        cancellation.check()?;
        let snapshot = crate::comparison_snapshots::insert(&self.id(), bytes, permit)?;
        if let Err(error) = cancellation.check() {
            if let Some(id) = snapshot["id"].as_str() {
                let _ = crate::comparison_snapshots::release_owned(id);
            }
            return Err(error);
        }
        Ok(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use std::{fs, process::Command};

    #[tokio::test]
    async fn image_snapshot_preserves_bytes_and_releases_failed_reads() {
        let root = std::env::temp_dir().join(format!("desktop-image-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        assert!(
            Command::new("git")
                .current_dir(&root)
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        let image = vec![0, 255, 0, 128, 42, 10];
        fs::write(root.join("[a].png"), &image).unwrap();
        fs::write(root.join("a.png"), b"not the image").unwrap();
        let huge = fs::File::create(root.join("huge.png")).unwrap();
        huge.set_len(crate::comparison_snapshots::MAX_BYTES as u64 + 1)
            .unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("[a].png", root.join("link.png")).unwrap();
            std::os::unix::fs::symlink(".", root.join("linkdir")).unwrap();
            std::os::unix::fs::symlink("/etc/passwd", root.join("outside.png")).unwrap();
            std::os::unix::fs::symlink(".git/config", root.join("metadata.png")).unwrap();
            std::os::unix::fs::symlink(root.join("[a].png"), root.join("absolute.png")).unwrap();
        }
        let repo = Repository::open(&root).await.unwrap();
        let owner = crate::comparison_snapshots::Owner::new();
        crate::comparison_snapshots::scope(owner.clone(), async {
            let snapshot = repo
                .renderer_working_file(GitPath::Text("[a].png".into()), None)
                .await
                .unwrap();
            let id = uuid::Uuid::parse_str(snapshot["id"].as_str().unwrap()).unwrap();
            let chunk = crate::comparison_snapshots::chunk(&repo.id(), id, 0, 100).unwrap();
            assert_eq!(
                STANDARD.decode(chunk["data"].as_str().unwrap()).unwrap(),
                image
            );
            crate::comparison_snapshots::release(&repo.id(), id).unwrap();
            let snapshot = repo
                .renderer_working_file(GitPath::Text("[a].png".into()), Some(image.len() as u64))
                .await
                .unwrap();
            let id = uuid::Uuid::parse_str(snapshot["id"].as_str().unwrap()).unwrap();
            crate::comparison_snapshots::release(&repo.id(), id).unwrap();
            assert_eq!(
                repo.renderer_working_file(
                    GitPath::Text("[a].png".into()),
                    Some(image.len() as u64 - 1)
                )
                .await
                .unwrap()["kind"],
                "too_large"
            );
            assert!(
                repo.renderer_working_file(
                    GitPath::Text("[a].png".into()),
                    Some(crate::comparison_snapshots::MAX_BYTES as u64 + 1)
                )
                .await
                .is_err()
            );
            for path in ["missing.png", "huge.png", "../outside.png"] {
                assert!(
                    repo.renderer_working_file(GitPath::Text(path.into()), None)
                        .await
                        .is_err()
                );
                drop(crate::comparison_snapshots::reserve().await.unwrap());
            }
            #[cfg(unix)]
            {
                for path in ["link.png", "linkdir/[a].png"] {
                    let snapshot = repo
                        .renderer_working_file(GitPath::Text(path.into()), None)
                        .await
                        .unwrap();
                    let id = uuid::Uuid::parse_str(snapshot["id"].as_str().unwrap()).unwrap();
                    let chunk = crate::comparison_snapshots::chunk(&repo.id(), id, 0, 100).unwrap();
                    assert_eq!(
                        STANDARD.decode(chunk["data"].as_str().unwrap()).unwrap(),
                        image
                    );
                    crate::comparison_snapshots::release(&repo.id(), id).unwrap();
                }
                for path in ["outside.png", "metadata.png", "absolute.png"] {
                    assert!(
                        repo.renderer_working_file(GitPath::Text(path.into()), None)
                            .await
                            .is_err()
                    );
                    drop(crate::comparison_snapshots::reserve().await.unwrap());
                }
            }
            let cancellation = crate::cancellation::Cancellation::default();
            cancellation.cancel();
            assert!(
                cancellation
                    .scope(repo.renderer_working_file(GitPath::Text("[a].png".into()), None))
                    .await
                    .is_err()
            );
            drop(crate::comparison_snapshots::reserve().await.unwrap());
        })
        .await;
        owner.close();
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    #[ignore = "reads a 70 MB image fixture"]
    async fn image_snapshot_accepts_exactly_the_size_limit() {
        let root =
            std::env::temp_dir().join(format!("desktop-image-limit-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        assert!(
            Command::new("git")
                .current_dir(&root)
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        let image = fs::File::create(root.join("large.png")).unwrap();
        image
            .set_len(crate::comparison_snapshots::MAX_BYTES as u64)
            .unwrap();
        let repo = Repository::open(&root).await.unwrap();
        let owner = crate::comparison_snapshots::Owner::new();
        crate::comparison_snapshots::scope(owner.clone(), async {
            let snapshot = repo
                .renderer_working_file(GitPath::Text("large.png".into()), None)
                .await
                .unwrap();
            assert_eq!(
                snapshot["size"],
                crate::comparison_snapshots::MAX_BYTES as u64
            );
            let id = uuid::Uuid::parse_str(snapshot["id"].as_str().unwrap()).unwrap();
            let tail = crate::comparison_snapshots::chunk(
                &repo.id(),
                id,
                crate::comparison_snapshots::MAX_BYTES as u64 - 1,
                1,
            )
            .unwrap();
            assert_eq!(
                STANDARD.decode(tail["data"].as_str().unwrap()).unwrap(),
                [0]
            );
            crate::comparison_snapshots::release(&repo.id(), id).unwrap();
        })
        .await;
        owner.close();
        fs::remove_dir_all(root).unwrap();
    }
}
