//! Reviewed named-remote fetch. Git and independent writers may race the final
//! review; interrupted fetches can already have updated refs or FETCH_HEAD.
use crate::{RendererFetchRequest, Repository, args, process};
use anyhow::Result;
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
#[cfg(any(unix, windows))]
use std::io::Read;

#[cfg(windows)]
fn windows_file_identity(file: &std::fs::File) -> Result<(u32, u64, u64, u32)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, GetFileInformationByHandle,
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: file owns a live handle and info is a valid writable structure.
    let success = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) };
    anyhow::ensure!(success != 0, "Cannot inspect FETCH_HEAD identity");
    anyhow::ensure!(
        info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0,
        "Cannot fetch into a reparse point"
    );
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    let write_time = (u64::from(info.ftLastWriteTime.dwHighDateTime) << 32)
        | u64::from(info.ftLastWriteTime.dwLowDateTime);
    Ok((
        info.dwVolumeSerialNumber,
        index,
        write_time,
        info.nNumberOfLinks,
    ))
}

fn fetch_head_certificate(directory: &std::path::Path) -> Result<Vec<u8>> {
    let dir = Dir::open_ambient_dir(directory, ambient_authority())?;
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = match dir.open_with("FETCH_HEAD", &options) {
        Ok(file) => file.into_std(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![0]),
        Err(_) => anyhow::bail!("Cannot safely inspect FETCH_HEAD"),
    };
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file() && metadata.len() <= 8 * 1024 * 1024,
        "Cannot safely inspect FETCH_HEAD"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(
            metadata.nlink() == 1,
            "Cannot fetch into a multiply linked FETCH_HEAD"
        );
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        anyhow::ensure!(
            bytes.len() as u64 == metadata.len(),
            "FETCH_HEAD changed during review"
        );
        let reopened = dir.open_with("FETCH_HEAD", &options)?.into_std();
        let after = reopened.metadata()?;
        anyhow::ensure!(
            after.is_file()
                && after.nlink() == 1
                && after.dev() == metadata.dev()
                && after.ino() == metadata.ino()
                && after.len() == metadata.len()
                && after.mtime() == metadata.mtime()
                && after.mtime_nsec() == metadata.mtime_nsec(),
            "FETCH_HEAD changed during review"
        );
        let mut hash = Sha256::new();
        hash.update([1]);
        hash.update(metadata.dev().to_le_bytes());
        hash.update(metadata.ino().to_le_bytes());
        hash.update(bytes);
        Ok(hash.finalize().to_vec())
    }
    #[cfg(windows)]
    {
        let identity = windows_file_identity(&file)?;
        anyhow::ensure!(
            identity.3 == 1,
            "Cannot fetch into a multiply linked FETCH_HEAD"
        );
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        anyhow::ensure!(
            bytes.len() as u64 == metadata.len(),
            "FETCH_HEAD changed during review"
        );
        let reopened = dir.open_with("FETCH_HEAD", &options)?.into_std();
        let after = reopened.metadata()?;
        anyhow::ensure!(
            after.is_file()
                && after.len() == metadata.len()
                && windows_file_identity(&reopened)? == identity,
            "FETCH_HEAD changed during review"
        );
        let mut hash = Sha256::new();
        hash.update([1]);
        hash.update(identity.0.to_le_bytes());
        hash.update(identity.1.to_le_bytes());
        hash.update(bytes);
        Ok(hash.finalize().to_vec())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        anyhow::bail!("Existing FETCH_HEAD requires platform-specific hardlink validation")
    }
}

impl Repository {
    async fn renderer_fetch_review(&self, request: &RendererFetchRequest) -> Result<String> {
        for variable in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_COMMON_DIR",
            "GIT_INDEX_FILE",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_NAMESPACE",
            "GIT_SHALLOW_FILE",
        ] {
            anyhow::ensure!(
                std::env::var_os(variable).is_none(),
                "Cannot review fetch with {variable} set"
            );
        }
        anyhow::ensure!(
            !request.remote.is_empty()
                && request.remote.len() <= 4096
                && !request.remote.chars().any(char::is_control),
            "Invalid configured remote"
        );
        let reference = format!("refs/remotes/{}/placeholder", request.remote);
        let format = self.run(&args(&["check-ref-format", &reference])).await?;
        anyhow::ensure!(format.code == 0, "Invalid configured remote");
        anyhow::ensure!(
            crate::remote_discovery::safe_url(&request.expected_url),
            "Remote URL cannot be safely reviewed"
        );
        anyhow::ensure!(
            self.fetch_remote_names().await?.contains(&request.remote),
            "Configured remote no longer exists"
        );
        let url = self
            .run(&args(&[
                "-c",
                "transfer.credentialsInUrl=allow",
                "ls-remote",
                "--get-url",
                "--",
                &request.remote,
            ]))
            .await?;
        anyhow::ensure!(
            url.code == 0
                && url.stdout.strip_suffix(b"\n") == Some(request.expected_url.as_bytes()),
            "Remote changed; refresh before fetching"
        );
        let refs = self
            .run(&args(&[
                "for-each-ref",
                "--format=%(refname)%00%(objectname)%00%(symref)",
            ]))
            .await?;
        anyhow::ensure!(refs.code == 0, "Cannot inspect repository refs");
        let config = self.checkout_configuration().await?;
        let worktrees = self.worktrees().await?;
        let fetch_head = fetch_head_certificate(&self.git_directory().await?)?;
        let mut hash = Sha256::new();
        for part in [
            self.id().as_bytes(),
            &serde_json::to_vec(request)?,
            &refs.stdout,
            &config,
            &serde_json::to_vec(&worktrees)?,
            &fetch_head,
        ] {
            hash.update((part.len() as u64).to_le_bytes());
            hash.update(part);
        }
        Ok(format!("{:x}", hash.finalize()))
    }

    pub(crate) async fn renderer_fetch_preview(
        &self,
        request: &RendererFetchRequest,
    ) -> Result<Value> {
        Ok(
            json!({"remote":request.remote,"recurse_submodules":request.recurse_submodules,"basis":self.renderer_fetch_review(request).await?}),
        )
    }

    pub(crate) async fn renderer_fetch(
        &self,
        request: &RendererFetchRequest,
        basis: &str,
    ) -> Result<Value> {
        let _lock = self.stash_lock().await?;
        anyhow::ensure!(
            self.renderer_fetch_review(request).await? == basis,
            "Fetch context changed; refresh before continuing"
        );
        let recurse = if request.recurse_submodules {
            "--recurse-submodules=on-demand"
        } else {
            "--recurse-submodules=no"
        };
        let output = process::run_bound_remote_fetch(
            &self.path,
            &args(&["fetch", "--prune", recurse, "--", &request.remote]),
            request.is_background_task,
        )
        .await?;
        Ok(json!({
            "code": output.code,
            "error_kind": if output.code == 0 {
                None
            } else {
                crate::fetch_error::classify(&output.stderr, &output.stdout)
            }
        }))
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::fetch_head_certificate;
    use std::fs;

    #[test]
    fn checks_existing_fetch_head_and_rejects_hard_links() {
        let dir = std::env::temp_dir().join(format!("desktop-fetch-head-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let head = dir.join("FETCH_HEAD");
        assert_eq!(fetch_head_certificate(&dir).unwrap(), vec![0]);
        fs::write(&head, b"first\n").unwrap();
        let first = fetch_head_certificate(&dir).unwrap();
        assert_eq!(first, fetch_head_certificate(&dir).unwrap());
        fs::write(&head, b"second\n").unwrap();
        assert_ne!(first, fetch_head_certificate(&dir).unwrap());
        let link = dir.join("hard-link");
        fs::hard_link(&head, &link).unwrap();
        assert!(fetch_head_certificate(&dir).is_err());
        fs::remove_file(link).unwrap();
        assert!(fetch_head_certificate(&dir).is_ok());
        fs::remove_file(&head).unwrap();
        let target = dir.join("target");
        fs::write(&target, b"sentinel").unwrap();
        // Windows may require developer mode or privileges for symlink creation.
        if std::os::windows::fs::symlink_file(&target, &head).is_ok() {
            assert!(fetch_head_certificate(&dir).is_err());
            assert_eq!(fs::read(&target).unwrap(), b"sentinel");
        }
        fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs, path::Path, process::Command};

    fn git(dir: &Path, args: &[&str]) -> String {
        let result = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout).unwrap().trim().to_owned()
    }

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("desktop-fetch-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(root.join("source")).unwrap();
            fs::create_dir(root.join("target")).unwrap();
            for dir in [root.join("source"), root.join("target")] {
                git(&dir, &["init", "-q", "-b", "main"]);
            }
            let source = root.join("source");
            git(&source, &["config", "user.name", "Test"]);
            git(&source, &["config", "user.email", "test@example.invalid"]);
            git(&source, &["config", "commit.gpgsign", "false"]);
            fs::write(source.join("file"), "base\n").unwrap();
            git(&source, &["add", "file"]);
            git(&source, &["commit", "-qm", "base"]);
            Self(root)
        }
        fn remote(&self, name: &str, url: &str) -> RendererFetchRequest {
            git(&self.0.join("target"), &["remote", "add", "--", name, url]);
            RendererFetchRequest {
                remote: name.into(),
                expected_url: url.into(),
                recurse_submodules: false,
                is_background_task: false,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn fetches_named_local_and_file_remotes() {
        for name in ["--test", "Tópico", "foo/bar"] {
            let f = Fixture::new();
            let source = f.0.join("source");
            let url = if name == "foo/bar" {
                format!("file://{}", source.display())
            } else {
                source.to_string_lossy().into_owned()
            };
            let request = f.remote(name, &url);
            let repo = Repository::open(&f.0.join("target")).await.unwrap();
            let preview = repo.renderer_fetch_preview(&request).await.unwrap();
            repo.renderer_fetch(&request, preview["basis"].as_str().unwrap())
                .await
                .unwrap();
            assert_eq!(
                git(
                    &f.0.join("target"),
                    &["rev-parse", &format!("refs/remotes/{name}/main")]
                ),
                git(&source, &["rev-parse", "HEAD"])
            );
            assert!(f.0.join("target/.git/FETCH_HEAD").is_file());
        }
    }

    #[tokio::test]
    async fn rejects_stale_reviews_and_linked_fetch_heads() {
        use std::os::unix::fs::symlink;
        let f = Fixture::new();
        let target = f.0.join("target");
        let request = f.remote("origin", &f.0.join("source").to_string_lossy());
        let repo = Repository::open(&target).await.unwrap();
        let preview = repo.renderer_fetch_preview(&request).await.unwrap();
        git(&target, &["config", "fetch.prune", "false"]);
        assert!(
            repo.renderer_fetch(&request, preview["basis"].as_str().unwrap())
                .await
                .is_err()
        );
        let outside = f.0.join("outside");
        fs::write(&outside, "sentinel").unwrap();
        let fetch_head = target.join(".git/FETCH_HEAD");
        symlink(&outside, &fetch_head).unwrap();
        assert!(repo.renderer_fetch_preview(&request).await.is_err());
        fs::remove_file(&fetch_head).unwrap();
        fs::hard_link(&outside, &fetch_head).unwrap();
        assert!(repo.renderer_fetch_preview(&request).await.is_err());
        assert_eq!(fs::read_to_string(outside).unwrap(), "sentinel");
    }

    #[test]
    fn request_debug_does_not_expose_url_credentials() {
        let request = RendererFetchRequest {
            remote: "origin".into(),
            expected_url: "https://user:synthetic-secret@example.invalid/repo".into(),
            recurse_submodules: false,
            is_background_task: false,
        };
        assert!(!format!("{request:?}").contains("synthetic-secret"));
        assert!(!crate::remote_discovery::safe_url(&request.expected_url));
    }
}
