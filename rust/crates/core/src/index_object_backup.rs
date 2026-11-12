//! Retain staged Git blobs and LFS payloads for durable index recovery.
use crate::{
    Repository, args,
    cancellation::{self, Cancellation},
    discard_fs as work,
    lfs::{self, LfsObject},
    process,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SavedObject {
    pub(crate) oid: String,
    pub(crate) size: u64,
    pub(crate) sha256: String,
}

fn validate_oid(oid: &str, length: usize) -> Result<()> {
    anyhow::ensure!(
        matches!(length, 40 | 64)
            && oid.len() == length
            && oid.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid saved object ID"
    );
    Ok(())
}

impl Repository {
    /// Save the supplied index entries once per blob; gitlinks point into a
    /// different object database and do not have a blob to copy.
    pub(crate) async fn save_index_objects(
        &self,
        directory: &Path,
        entries: Vec<(String, String)>,
        available_only: bool,
    ) -> Result<(Vec<SavedObject>, Vec<LfsObject>, Vec<LfsObject>)> {
        let mut objects = Vec::<SavedObject>::new();
        let mut seen = std::collections::HashSet::new();
        let mut lfs = Vec::<LfsObject>::new();
        for (mode, oid) in entries {
            validate_oid(&oid, oid.len())?;
            if mode == "160000" || !seen.insert(oid.clone()) {
                continue;
            }
            let path = directory.join(format!("object-{oid}"));
            let output = process::git_to_file(
                &self.path,
                &args(&["--no-replace-objects", "cat-file", "blob", &oid]),
                &path,
            )
            .await?;
            anyhow::ensure!(
                output.code == 0,
                "cannot save staged Git blob: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let state =
                cancellation::blocking(move || work::copy_state(fs::File::open(path)?, None))
                    .await?;
            if state.size < 1024 {
                let bytes = fs::read(directory.join(format!("object-{oid}")))?;
                if let Some(pointer) = lfs::pointer(&bytes).filter(|pointer| pointer.size != 0) {
                    if let Some(existing) = lfs.iter().find(|object| object.oid == pointer.oid) {
                        anyhow::ensure!(*existing == pointer, "inconsistent LFS pointer sizes");
                    } else {
                        lfs.push(pointer);
                    }
                }
            }
            objects.push(SavedObject {
                oid,
                size: state.size,
                sha256: state.sha256,
            });
        }
        if !lfs.is_empty() && !available_only {
            lfs::save_payloads(
                self.lfs_media_directory().await?,
                directory.to_owned(),
                lfs.clone(),
            )
            .await?;
        }
        let mut unavailable = Vec::new();
        if available_only && !lfs.is_empty() {
            match self.lfs_media_directory().await {
                Ok(media) => {
                    let retained =
                        lfs::save_available_payloads(media, directory.to_owned(), lfs).await?;
                    lfs = retained.0;
                    unavailable = retained.1;
                }
                Err(_) => {
                    Cancellation::current().check()?;
                    unavailable = std::mem::take(&mut lfs);
                }
            }
        }
        Ok((objects, lfs, unavailable))
    }

    pub(crate) async fn restore_index_objects(
        &self,
        directory: &Path,
        objects: &[SavedObject],
        lfs: &[LfsObject],
        unavailable_lfs: &[LfsObject],
        hash_length: usize,
        verify_lfs_pointers: bool,
    ) -> Result<()> {
        for object in objects {
            validate_oid(&object.oid, hash_length)?;
            let path = directory.join(format!("object-{}", object.oid));
            let state = cancellation::blocking({
                let path = path.clone();
                move || work::copy_state(fs::File::open(path)?, None)
            })
            .await?;
            anyhow::ensure!(
                state.size == object.size && state.sha256 == object.sha256,
                "saved index blob was changed; undo data retained"
            );
            if verify_lfs_pointers
                && object.size < 1024
                && let Some(pointer) =
                    lfs::pointer(&fs::read(&path)?).filter(|pointer| pointer.size != 0)
            {
                anyhow::ensure!(
                    lfs.contains(&pointer) || unavailable_lfs.contains(&pointer),
                    "saved LFS payload is missing from the undo record"
                );
            }
            let mut command = args(&["hash-object", "-w", "--no-filters", "--"]);
            command.push(path.into_os_string());
            let restored = self.run(&command).await?;
            anyhow::ensure!(
                restored.code == 0,
                "cannot restore saved index blob: {}",
                String::from_utf8_lossy(&restored.stderr)
            );
            anyhow::ensure!(
                std::str::from_utf8(&restored.stdout)
                    .context("invalid restored object ID")?
                    .trim()
                    == object.oid,
                "saved index blob no longer matches its Git object ID"
            );
        }
        if !lfs.is_empty() {
            lfs::restore_payloads(
                self.lfs_media_directory().await?,
                directory.to_owned(),
                lfs.to_vec(),
            )
            .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn retained_blobs_survive_pruning_and_corruption_is_rejected() {
        let root =
            std::env::temp_dir().join(format!("desktop-index-objects-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let git = |arguments: &[&str]| {
            let result = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(arguments)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            String::from_utf8(result.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "-q"]);
        fs::write(root.join("file"), b"staged only\n").unwrap();
        let oid = git(&["hash-object", "-w", "file"]);
        let backup = root.join("backup");
        fs::create_dir(&backup).unwrap();
        let repo = Repository::open(&root).await.unwrap();
        let (objects, lfs, unavailable) = repo
            .save_index_objects(
                &backup,
                vec![
                    ("100644".into(), oid.clone()),
                    ("100755".into(), oid.clone()),
                    ("160000".into(), oid.clone()),
                ],
                false,
            )
            .await
            .unwrap();
        assert_eq!(objects.len(), 1);
        assert!(lfs.is_empty() && unavailable.is_empty());
        assert!(
            repo.save_index_objects(&backup, vec![("100644".into(), "../invalid".into())], false)
                .await
                .is_err()
        );
        git(&["prune", "--expire", "now"]);
        let exists = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["cat-file", "-e", &oid])
            .status()
            .unwrap();
        assert!(!exists.success());
        repo.restore_index_objects(&backup, &objects, &lfs, &unavailable, oid.len(), true)
            .await
            .unwrap();
        assert_eq!(git(&["cat-file", "blob", &oid]), "staged only");
        let pointer = format!(
            "version https://git-lfs.github.com/spec/v1\noid sha256:{}\nsize 3\n",
            "a".repeat(64)
        );
        fs::write(root.join("pointer"), pointer).unwrap();
        let pointer_oid = git(&["hash-object", "-w", "pointer"]);
        let pointer_backup = root.join("pointer-backup");
        fs::create_dir(&pointer_backup).unwrap();
        let (pointer_objects, retained, unavailable) = repo
            .save_index_objects(
                &pointer_backup,
                vec![("100644".into(), pointer_oid.clone())],
                true,
            )
            .await
            .unwrap();
        assert!(retained.is_empty());
        assert_eq!(unavailable.len(), 1);
        assert!(
            repo.restore_index_objects(
                &pointer_backup,
                &pointer_objects,
                &[],
                &[],
                pointer_oid.len(),
                true
            )
            .await
            .is_err()
        );
        repo.restore_index_objects(
            &pointer_backup,
            &pointer_objects,
            &retained,
            &unavailable,
            pointer_oid.len(),
            true,
        )
        .await
        .unwrap();
        fs::write(backup.join(format!("object-{oid}")), b"tampered").unwrap();
        assert!(
            repo.restore_index_objects(&backup, &objects, &lfs, &unavailable, oid.len(), true)
                .await
                .is_err()
        );
        assert_eq!(git(&["cat-file", "blob", &oid]), "staged only");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn staged_lfs_payload_is_retained_and_restored() {
        use sha2::{Digest, Sha256};
        let root = std::env::temp_dir().join(format!("desktop-index-lfs-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let git = |arguments: &[&str]| {
            let result = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(arguments)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            String::from_utf8(result.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "-q"]);
        let storage = root.join("fake-lfs");
        let media = storage.join("objects");
        let payload = b"retained LFS payload\n";
        let payload_oid = format!("{:x}", Sha256::digest(payload));
        let target = media
            .join(&payload_oid[..2])
            .join(&payload_oid[2..4])
            .join(&payload_oid);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, payload).unwrap();
        let alias = format!(
            "!f() {{ printf 'LocalMediaDir=%s\\nLfsStorageDir=%s\\n' '{}' '{}'; }}; f",
            media.display(),
            storage.display()
        );
        git(&["config", "alias.lfs", &alias]);
        let pointer = format!(
            "version https://git-lfs.github.com/spec/v1\noid sha256:{payload_oid}\nsize {}\n",
            payload.len()
        );
        fs::write(root.join("pointer"), pointer).unwrap();
        let pointer_oid = git(&["hash-object", "-w", "pointer"]);
        let backup = root.join("backup");
        fs::create_dir(&backup).unwrap();
        let repo = Repository::open(&root).await.unwrap();
        let (objects, retained, unavailable) = repo
            .save_index_objects(&backup, vec![("100644".into(), pointer_oid.clone())], false)
            .await
            .unwrap();
        assert_eq!(retained.len(), 1);
        assert!(unavailable.is_empty());
        assert_eq!(
            fs::read(backup.join(format!("lfs-{payload_oid}"))).unwrap(),
            payload
        );
        fs::remove_file(&target).unwrap();
        repo.restore_index_objects(
            &backup,
            &objects,
            &retained,
            &unavailable,
            pointer_oid.len(),
            true,
        )
        .await
        .unwrap();
        assert_eq!(fs::read(&target).unwrap(), payload);
        fs::remove_dir_all(root).unwrap();
    }
}
