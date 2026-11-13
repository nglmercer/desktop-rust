//! Session-owned conflict-resolution previews without renderer filesystem access.
use crate::{GitPath, Repository, args, process};
use anyhow::{Context, Result};
use cap_std::{ambient_authority, fs::Dir};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

fn decode_bounded(mut bytes: &[u8], limit: usize) -> Result<Vec<u8>> {
    let mut result = Vec::with_capacity(bytes.len().min(limit));
    while !bytes.is_empty() {
        crate::cancellation::Cancellation::current().check()?;
        match std::str::from_utf8(bytes) {
            Ok(valid) => {
                anyhow::ensure!(
                    result.len().saturating_add(valid.len()) <= limit,
                    "Resolution content exceeds the supported size"
                );
                result.extend_from_slice(valid.as_bytes());
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                anyhow::ensure!(
                    result.len().saturating_add(valid).saturating_add(3) <= limit,
                    "Resolution content exceeds the supported size"
                );
                result.extend_from_slice(&bytes[..valid]);
                result.extend_from_slice("�".as_bytes());
                bytes = &bytes[valid + error.error_len().unwrap_or(bytes.len() - valid)..];
            }
        }
    }
    Ok(result)
}

fn write_preview(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = crate::discard_fs::private_file(path)?;
    for chunk in bytes.chunks(64 * 1024) {
        crate::cancellation::Cancellation::current().check()?;
        file.write_all(chunk)?;
    }
    file.sync_all()?;
    Ok(())
}

struct TemporaryDirectory(PathBuf);
impl TemporaryDirectory {
    fn create() -> Result<Self> {
        let root = std::env::temp_dir().canonicalize()?;
        let path = root.join(format!("desktop-resolution-{}", uuid::Uuid::new_v4()));
        crate::lfs_progress::create_private_directory(&path)?;
        Ok(Self(path))
    }
}
impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Repository {
    async fn resolution_stage(&self, path: &std::ffi::OsStr, stage: u8) -> Result<Vec<u8>> {
        let mut command = args(&["--literal-pathspecs", "ls-files", "--stage", "-z", "--"]);
        command.push(path.to_owned());
        let entries = self.run(&command).await?;
        anyhow::ensure!(entries.code == 0, "Cannot inspect the conflict index");
        let mut found: Option<String> = None;
        for entry in entries
            .stdout
            .split(|byte| *byte == 0)
            .filter(|entry| !entry.is_empty())
        {
            let tab = entry
                .iter()
                .position(|byte| *byte == b'\t')
                .context("Invalid conflict index entry")?;
            if &entry[tab + 1..] != path.as_encoded_bytes() {
                continue;
            }
            let fields: Vec<_> = std::str::from_utf8(&entry[..tab])?
                .split_whitespace()
                .collect();
            anyhow::ensure!(fields.len() == 3, "Invalid conflict index entry");
            if fields[2] != stage.to_string() {
                continue;
            }
            let oid = fields[1];
            anyhow::ensure!(
                found.is_none()
                    && matches!(oid.len(), 40 | 64)
                    && oid.bytes().all(|b| b.is_ascii_hexdigit()),
                "Invalid conflict blob"
            );
            found = Some(oid.to_owned());
        }
        let Some(oid) = found else {
            return Ok(Vec::new());
        };
        let size = self.run(&args(&["cat-file", "-s", &oid])).await?;
        anyhow::ensure!(size.code == 0, "Cannot inspect conflict blob size");
        let expected: u64 = std::str::from_utf8(&size.stdout)?.trim().parse()?;
        anyhow::ensure!(
            expected <= crate::comparison_snapshots::MAX_BYTES as u64,
            "Conflict blob exceeds the supported size"
        );
        let output =
            process::run_resolution_blob(&self.path, &args(&["cat-file", "blob", &oid])).await?;
        anyhow::ensure!(
            output.code == 0 && output.stdout.len() <= crate::comparison_snapshots::MAX_BYTES,
            "Cannot read conflict blob"
        );
        anyhow::ensure!(
            output.stdout.len() as u64 == expected,
            "Conflict blob changed during preview"
        );
        Ok(output.stdout)
    }
    pub(crate) async fn renderer_resolution_diff(
        &self,
        path: GitPath,
        stage: Option<u8>,
        hide_whitespace: bool,
        snapshot_id: &str,
        size: u64,
    ) -> Result<serde_json::Value> {
        anyhow::ensure!(
            stage.is_none_or(|value| value == 2 || value == 3),
            "Invalid conflict stage"
        );
        let checked = crate::discard_fs::checked_path(path.clone())?;
        let (uploaded, permit) = crate::comparison_snapshots::take_upload(
            &self.id(),
            uuid::Uuid::parse_str(snapshot_id)?,
            size,
        )?;
        anyhow::ensure!(
            stage.is_none() || uploaded.is_empty(),
            "Conflict stage must not include uploaded content"
        );
        let root_path = self.path.clone();
        let checked_stage = checked.clone();
        let base = crate::cancellation::blocking(move || {
            let root = Dir::open_ambient_dir(&root_path, ambient_authority())?;
            let file = crate::discard_fs::WorkingFile::open(&root, Path::new(&checked), false)?
                .context("Resolution file does not exist")?;
            let bytes = file.read_bounded(crate::comparison_snapshots::MAX_BYTES as u64)?;
            decode_bounded(&bytes, crate::comparison_snapshots::MAX_BYTES)
        })
        .await?;
        let target = if let Some(stage) = stage {
            let bytes = self.resolution_stage(&checked_stage, stage).await?;
            crate::cancellation::blocking(move || {
                decode_bounded(&bytes, crate::comparison_snapshots::MAX_BYTES)
            })
            .await?
        } else {
            String::from_utf8(uploaded)
                .context("Invalid resolution content encoding")?
                .into_bytes()
        };
        anyhow::ensure!(
            base.len().saturating_add(target.len()) <= crate::comparison_snapshots::MAX_BYTES,
            "Resolution content exceeds the supported size"
        );
        let (_directory, before, after, base, target) = crate::cancellation::blocking(move || {
            let directory = TemporaryDirectory::create()?;
            let before = directory.0.join("before");
            let after = directory.0.join("after");
            write_preview(&before, &base)?;
            write_preview(&after, &target)?;
            Ok((directory, before, after, base, target))
        })
        .await?;
        let mut command = args(&["diff"]);
        if hide_whitespace {
            command.push("-w".into());
        }
        command.extend(args(&[
            "--no-ext-diff",
            "--patch-with-raw",
            "-z",
            "--no-color",
            "--no-index",
            "--",
        ]));
        command.push(before.into_os_string());
        command.push(after.into_os_string());
        let output = process::run_comparison_snapshot(&self.path, &command).await?;
        anyhow::ensure!(
            matches!(output.code, 0 | 1),
            "Could not create resolution preview"
        );
        let total = 8_usize
            .saturating_add(base.len())
            .saturating_add(target.len())
            .saturating_add(output.stdout.len());
        anyhow::ensure!(
            total <= crate::comparison_snapshots::MAX_BYTES,
            "Resolution preview exceeds the supported size"
        );
        let mut bytes = Vec::with_capacity(total);
        bytes.extend_from_slice(&(base.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&(target.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&base);
        bytes.extend_from_slice(&target);
        bytes.extend_from_slice(&output.stdout);
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
    #[test]
    fn malformed_utf8_decoding_respects_output_limit() {
        assert_eq!(decode_bounded(&[0xff, b'a'], 4).unwrap(), "�a".as_bytes());
        assert!(decode_bounded(&[0xff, b'a'], 3).is_err());
        assert_eq!(decode_bounded(&[0xe2, 0x82], 3).unwrap(), "�".as_bytes());
    }
    #[tokio::test]
    async fn cancelled_preview_files_are_removed() {
        let cancellation = crate::cancellation::Cancellation::default();
        cancellation.cancel();
        let directory = TemporaryDirectory::create().unwrap();
        let path = directory.0.clone();
        let result = cancellation
            .scope(async { write_preview(&path.join("before"), b"content") })
            .await;
        assert!(result.is_err());
        drop(directory);
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_worktree_symlinks_and_releases_failed_uploads() {
        use std::os::unix::fs::symlink;
        let root =
            std::env::temp_dir().join(format!("desktop-resolution-links-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        assert!(
            Command::new("git")
                .current_dir(&root)
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        symlink("/etc/passwd", root.join("link")).unwrap();
        let repository = Repository::open(&root).await.unwrap();
        let owner = crate::comparison_snapshots::Owner::new();
        crate::comparison_snapshots::scope(owner.clone(), async {
            let permit = crate::comparison_snapshots::reserve().await.unwrap();
            let upload =
                crate::comparison_snapshots::insert_upload(&repository.id(), permit).unwrap();
            assert!(
                repository
                    .renderer_resolution_diff(
                        GitPath::Text("link".into()),
                        None,
                        false,
                        upload["id"].as_str().unwrap(),
                        0
                    )
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
    #[ignore = "creates a 70 MB Git blob"]
    async fn rejects_an_oversized_conflict_blob_before_preview() {
        use std::io::Write as _;
        use std::process::Stdio;
        let root =
            std::env::temp_dir().join(format!("desktop-resolution-large-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        assert!(
            Command::new("git")
                .current_dir(&root)
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        fs::write(
            root.join("large"),
            vec![b'x'; crate::comparison_snapshots::MAX_BYTES + 1],
        )
        .unwrap();
        let output = Command::new("git")
            .current_dir(&root)
            .args(["hash-object", "-w", "large"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let oid = String::from_utf8(output.stdout).unwrap().trim().to_owned();
        let mut update = Command::new("git")
            .current_dir(&root)
            .args(["update-index", "--index-info"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        writeln!(update.stdin.take().unwrap(), "100644 {oid} 2\tlarge").unwrap();
        assert!(update.wait().unwrap().success());
        let repository = Repository::open(&root).await.unwrap();
        assert!(
            repository
                .resolution_stage(std::ffi::OsStr::new("large"), 2)
                .await
                .unwrap_err()
                .to_string()
                .contains("exceeds")
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn previews_literal_conflict_stages_and_uploaded_content() {
        let root =
            std::env::temp_dir().join(format!("desktop-resolution-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let git = |arguments: &[&str]| {
            let output = Command::new("git")
                .current_dir(&root)
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
        fs::write(root.join("[a].txt"), "base\n").unwrap();
        git(&["add", "--", "."]);
        git(&["commit", "-qm", "base"]);
        git(&["checkout", "-qb", "other"]);
        fs::write(root.join("[a].txt"), "theirs\n").unwrap();
        git(&["commit", "-qam", "theirs"]);
        git(&["checkout", "-q", "main"]);
        fs::write(root.join("[a].txt"), "ours\n").unwrap();
        git(&["commit", "-qam", "ours"]);
        assert!(
            !Command::new("git")
                .current_dir(&root)
                .args(["merge", "other"])
                .output()
                .unwrap()
                .status
                .success()
        );
        // Include invalid UTF-8 to verify Node-compatible replacement characters.
        fs::write(root.join("[a].txt"), b"\xef\xbb\xbfworking \xff\n").unwrap();
        let repository = Repository::open(&root).await.unwrap();
        let owner = crate::comparison_snapshots::Owner::new();
        crate::comparison_snapshots::scope(owner.clone(), async {
            let repo = &repository;
            let run = |stage, content: Vec<u8>| async move {
                let permit = crate::comparison_snapshots::reserve().await.unwrap();
                let upload =
                    crate::comparison_snapshots::insert_upload(&repo.id(), permit).unwrap();
                let id = upload["id"].as_str().unwrap();
                crate::comparison_snapshots::append_upload(
                    &repo.id(),
                    uuid::Uuid::parse_str(id).unwrap(),
                    0,
                    &STANDARD.encode(&content),
                )
                .unwrap();
                let snapshot = repo
                    .renderer_resolution_diff(
                        GitPath::Text("[a].txt".into()),
                        stage,
                        false,
                        id,
                        content.len() as u64,
                    )
                    .await
                    .unwrap();
                let snapshot_id = uuid::Uuid::parse_str(snapshot["id"].as_str().unwrap()).unwrap();
                let chunk =
                    crate::comparison_snapshots::chunk(&repo.id(), snapshot_id, 0, 512 * 1024)
                        .unwrap();
                let bytes = STANDARD.decode(chunk["data"].as_str().unwrap()).unwrap();
                crate::comparison_snapshots::release(&repo.id(), snapshot_id).unwrap();
                let old_len = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
                let new_len = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
                (
                    String::from_utf8(bytes[8..8 + old_len].to_vec()).unwrap(),
                    String::from_utf8(bytes[8 + old_len..8 + old_len + new_len].to_vec()).unwrap(),
                    bytes[8 + old_len + new_len..].to_vec(),
                )
            };
            let (old, new, diff) = run(Some(2), Vec::new()).await;
            assert_eq!(old, "\u{feff}working �\n");
            assert_eq!(new, "ours\n");
            assert!(String::from_utf8_lossy(&diff).contains("+ours"));
            let (_, new, _) = run(Some(3), Vec::new()).await;
            assert_eq!(new, "theirs\n");
            let (_, new, _) = run(None, "chosen ✓\n".as_bytes().to_vec()).await;
            assert_eq!(new, "chosen ✓\n");
            git(&[
                "--literal-pathspecs",
                "rm",
                "--cached",
                "-q",
                "--",
                "[a].txt",
            ]);
            let (_, new, _) = run(Some(3), Vec::new()).await;
            assert_eq!(new, "");
            let permit = crate::comparison_snapshots::reserve().await.unwrap();
            let upload =
                crate::comparison_snapshots::insert_upload(&repository.id(), permit).unwrap();
            let id = upload["id"].as_str().unwrap();
            assert!(
                repository
                    .renderer_resolution_diff(GitPath::Text("missing".into()), None, false, id, 0)
                    .await
                    .is_err()
            );
            // A failed read consumes its upload and releases the session slot.
            drop(crate::comparison_snapshots::reserve().await.unwrap());
        })
        .await;
        owner.close();
        fs::remove_dir_all(root).unwrap();
    }
}
