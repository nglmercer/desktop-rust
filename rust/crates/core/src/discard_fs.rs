//! Directory-handle based working-file replacement; parent symlinks are never followed.
use crate::{GitPath, cancellation::Cancellation, os_from_bytes, validate_path};
use anyhow::{Context, Result, bail};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs,
    io::{Read, Write},
    path::{Component, Path},
};

#[derive(Debug)]
pub(crate) struct WorkingFileTooLarge;

impl std::fmt::Display for WorkingFileTooLarge {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Working file exceeds the supported size")
    }
}

impl std::error::Error for WorkingFileTooLarge {}

#[cfg(test)]
thread_local! {
    static CAPTURE_TEST_AFTER_READ: std::cell::RefCell<Option<Box<dyn FnMut()>>> = std::cell::RefCell::new(None);
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    Absent,
    Regular,
    Symlink,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileState {
    pub kind: Kind,
    pub size: u64,
    pub sha256: String,
    pub permissions: u32,
}
impl FileState {
    pub fn absent() -> Self {
        Self {
            kind: Kind::Absent,
            size: 0,
            sha256: String::new(),
            permissions: 0,
        }
    }
}

pub(crate) fn checked_path(path: GitPath) -> Result<OsString> {
    let path = validate_path(path)?;
    for component in Path::new(&path).components() {
        let name = component.as_os_str().to_string_lossy();
        if name
            .trim_end_matches(['.', ' '])
            .eq_ignore_ascii_case(".git")
            || name.contains(':') && cfg!(windows)
        {
            bail!(
                "Git metadata and special filesystem paths cannot be used for this file operation"
            );
        }
    }
    Ok(path)
}

pub(crate) struct WorkingFile {
    parent: Dir,
    name: OsString,
}
impl WorkingFile {
    pub fn open(root: &Dir, path: &Path, create: bool) -> Result<Option<Self>> {
        let mut components = path.components().peekable();
        let mut parent = root.try_clone()?;
        while let Some(component) = components.next() {
            let Component::Normal(name) = component else {
                bail!("invalid working-file path");
            };
            if components.peek().is_none() {
                return Ok(Some(Self {
                    parent,
                    name: name.to_owned(),
                }));
            }
            parent = match parent.open_dir_nofollow(name) {
                Ok(directory) => directory,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => {
                    parent.create_dir(name)?;
                    parent.open_dir_nofollow(name)?
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => {
                    return Err(error)
                        .context("working path traverses a symlink or inaccessible directory");
                }
            };
        }
        bail!("working file needs a filename")
    }

    pub fn capture(&self, backup: Option<&Path>) -> Result<FileState> {
        self.capture_with_limit(backup, None)
    }

    fn capture_with_limit(&self, backup: Option<&Path>, limit: Option<u64>) -> Result<FileState> {
        let metadata = match self.parent.symlink_metadata(&self.name) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(FileState::absent());
            }
            Err(error) => return Err(error.into()),
        };
        if metadata.is_dir() {
            bail!("directories and submodules require their own file workflow");
        }
        if metadata.file_type().is_symlink() {
            let target = self.parent.read_link_contents(&self.name)?;
            let bytes = target.as_os_str().as_encoded_bytes();
            if limit.is_some_and(|limit| bytes.len() as u64 > limit) {
                return Err(WorkingFileTooLarge.into());
            }
            if let Some(backup) = backup {
                private_write(backup, bytes)?;
            }
            return Ok(FileState {
                kind: Kind::Symlink,
                size: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(bytes)),
                permissions: 0o777,
            });
        }
        if !metadata.is_file() {
            bail!("special filesystem objects cannot be discarded");
        }
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = self.parent.open_with(&self.name, &options)?.into_std();
        copy_state_with_limit(file, backup, limit)
    }

    /// Read a regular worktree file without following symlinks or special files.
    pub fn read_bounded(&self, limit: u64) -> Result<Vec<u8>> {
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = self.parent.open_with(&self.name, &options)?.into_std();
        let metadata = file.metadata()?;
        anyhow::ensure!(
            metadata.is_file(),
            "Working file is not a supported regular file"
        );
        if metadata.len() > limit {
            return Err(WorkingFileTooLarge.into());
        }
        let mut input = file.take(limit + 1);
        let mut bytes = Vec::new();
        let mut chunk = [0; 64 * 1024];
        loop {
            Cancellation::current().check()?;
            let count = input.read(&mut chunk)?;
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..count]);
        }
        if bytes.len() as u64 > limit {
            return Err(WorkingFileTooLarge.into());
        }
        let after = input.into_inner().metadata()?;
        anyhow::ensure!(
            bytes.len() as u64 == metadata.len()
                && after.len() == metadata.len()
                && after.modified().ok() == metadata.modified().ok(),
            "Working file changed while reading"
        );
        let current = self.parent.symlink_metadata(&self.name)?;
        anyhow::ensure!(
            current.is_file() && current.len() == metadata.len(),
            "Working file changed while reading"
        );
        #[cfg(unix)]
        {
            use cap_std::fs::MetadataExt as CapMetadataExt;
            use std::os::unix::fs::MetadataExt as StdMetadataExt;
            anyhow::ensure!(
                CapMetadataExt::dev(&current) == StdMetadataExt::dev(&metadata)
                    && CapMetadataExt::ino(&current) == StdMetadataExt::ino(&metadata)
                    && StdMetadataExt::ctime(&after) == StdMetadataExt::ctime(&metadata)
                    && StdMetadataExt::ctime_nsec(&after) == StdMetadataExt::ctime_nsec(&metadata),
                "Working file changed while reading"
            );
        }
        Ok(bytes)
    }

    pub fn stage(&self, source: &Path, expected: &FileState) -> Result<StagedFile> {
        if expected.kind == Kind::Absent {
            return Ok(StagedFile {
                parent: self.parent.try_clone()?,
                temporary: None,
                target: self.name.clone(),
            });
        }
        let name = OsString::from(format!(".desktop-change-{}", uuid::Uuid::new_v4()));
        let mut staged = StagedFile {
            parent: self.parent.try_clone()?,
            temporary: None,
            target: self.name.clone(),
        };
        if expected.kind == Kind::Symlink {
            let bytes = fs::read(source)?;
            anyhow::ensure!(
                bytes.len() <= 65536 && format!("{:x}", Sha256::digest(&bytes)) == expected.sha256,
                "undo/restore data was changed"
            );
            let target = os_from_bytes(&bytes)?;
            #[cfg(not(windows))]
            self.parent.symlink_contents(target, &name)?;
            #[cfg(windows)]
            self.parent.symlink_file(target, &name)?;
            staged.temporary = Some(name.clone());
        } else {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use cap_std::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut destination = self.parent.open_with(&name, &options)?.into_std();
            staged.temporary = Some(name.clone());
            let mut input = fs::File::open(source)?;
            let mut digest = Sha256::new();
            let mut buffer = [0; 64 * 1024];
            let mut size = 0;
            loop {
                Cancellation::current().check()?;
                let count = input.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                destination.write_all(&buffer[..count])?;
                digest.update(&buffer[..count]);
                size += count as u64;
            }
            anyhow::ensure!(
                size == expected.size && format!("{:x}", digest.finalize()) == expected.sha256,
                "undo/restore data was changed"
            );
            set_permissions(&destination, expected.permissions)?;
            destination.sync_all()?;
        }
        // Keep the guard mutable so a failed preparation always removes its sibling.
        staged.temporary = Some(name);
        Ok(staged)
    }
}

pub(crate) struct StagedFile {
    parent: Dir,
    temporary: Option<OsString>,
    target: OsString,
}
impl StagedFile {
    pub fn publish(&mut self) -> Result<()> {
        if let Some(temporary) = &self.temporary {
            self.parent.rename(temporary, &self.parent, &self.target)?;
            self.temporary = None;
        } else {
            match self.parent.remove_file(&self.target) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        #[cfg(unix)]
        self.parent.open(".")?.into_std().sync_all()?;
        Ok(())
    }
}
impl Drop for StagedFile {
    fn drop(&mut self) {
        if let Some(temporary) = &self.temporary {
            let _ = self.parent.remove_file(temporary);
        }
    }
}

pub(crate) fn capture(root: &Dir, path: &Path, backup: Option<&Path>) -> Result<FileState> {
    match WorkingFile::open(root, path, false)? {
        Some(file) => file.capture(backup),
        None => Ok(FileState::absent()),
    }
}

pub(crate) fn capture_bounded(root: &Dir, path: &Path, limit: u64) -> Result<FileState> {
    match WorkingFile::open(root, path, false)? {
        Some(file) => file.capture_with_limit(None, Some(limit)),
        None => Ok(FileState::absent()),
    }
}

pub(crate) fn copy_state(file: fs::File, backup: Option<&Path>) -> Result<FileState> {
    copy_state_with_limit(file, backup, None)
}

#[cfg(test)]
mod bounded_capture_tests {
    use super::*;
    use cap_std::ambient_authority;

    #[test]
    fn rejects_oversized_regular_files_before_reading() {
        let directory =
            std::env::temp_dir().join(format!("desktop-bounded-capture-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("conflict");
        fs::write(&path, b"12345678").unwrap();
        let root = Dir::open_ambient_dir(&directory, ambient_authority()).unwrap();
        assert_eq!(
            capture_bounded(&root, Path::new("conflict"), 8)
                .unwrap()
                .size,
            8
        );
        assert!(
            capture_bounded(&root, Path::new("conflict"), 7)
                .unwrap_err()
                .is::<WorkingFileTooLarge>()
        );
        assert_eq!(
            capture_bounded(&root, Path::new("missing"), 0)
                .unwrap()
                .kind,
            Kind::Absent
        );
        drop(root);
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn cancellation_stops_hashing_after_the_current_chunk() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let directory =
            std::env::temp_dir().join(format!("desktop-cancel-capture-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        let file = fs::File::create(directory.join("conflict")).unwrap();
        file.set_len(10_485_761).unwrap();
        drop(file);
        let root = Dir::open_ambient_dir(&directory, ambient_authority()).unwrap();
        let token = Cancellation::default();
        let chunks = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&chunks);
        let result = token
            .scope(async move {
                crate::cancellation::blocking(move || {
                    CAPTURE_TEST_AFTER_READ.with(|hook| {
                        *hook.borrow_mut() = Some(Box::new(move || {
                            observed.fetch_add(1, Ordering::SeqCst);
                            Cancellation::current().cancel();
                        }));
                    });
                    let result = capture(&root, Path::new("conflict"), None);
                    CAPTURE_TEST_AFTER_READ.with(|hook| {
                        hook.borrow_mut().take();
                    });
                    result
                })
                .await
            })
            .await;
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Operation cancelled")
        );
        assert_eq!(chunks.load(Ordering::SeqCst), 1);
        fs::remove_dir_all(directory).unwrap();
    }
}

fn copy_state_with_limit(
    mut file: fs::File,
    backup: Option<&Path>,
    limit: Option<u64>,
) -> Result<FileState> {
    let before = file.metadata()?;
    if !before.is_file() {
        bail!("expected a regular file");
    }
    if limit.is_some_and(|limit| before.len() > limit) {
        return Err(WorkingFileTooLarge.into());
    }
    let mut output = backup.map(private_file).transpose()?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    let mut size = 0;
    loop {
        Cancellation::current().check()?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        if limit.is_some_and(|limit| size + count as u64 > limit) {
            return Err(WorkingFileTooLarge.into());
        }
        if let Some(output) = &mut output {
            output.write_all(&buffer[..count])?;
        }
        digest.update(&buffer[..count]);
        size += count as u64;
        #[cfg(test)]
        CAPTURE_TEST_AFTER_READ.with(|hook| {
            if let Some(hook) = hook.borrow_mut().as_mut() {
                hook();
            }
        });
    }
    if let Some(output) = output {
        output.sync_all()?;
    }
    let after = file.metadata()?;
    anyhow::ensure!(
        before.len() == after.len()
            && before.modified().ok() == after.modified().ok()
            && size == before.len(),
        "file changed while reading; refresh before retrying the operation"
    );
    #[cfg(unix)]
    let permissions = {
        use std::os::unix::fs::PermissionsExt;
        before.permissions().mode() & 0o777
    };
    #[cfg(not(unix))]
    let permissions = if before.permissions().readonly() {
        0o444
    } else {
        0o644
    };
    Ok(FileState {
        kind: Kind::Regular,
        size,
        sha256: format!("{:x}", digest.finalize()),
        permissions,
    })
}

pub(crate) fn private_file(path: &Path) -> Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}
pub(crate) fn private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = private_file(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
pub(crate) fn set_permissions(file: &fs::File, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(mode & 0o777))?;
    }
    #[cfg(not(unix))]
    {
        let mut permissions = file.metadata()?.permissions();
        permissions.set_readonly(mode & 0o200 == 0);
        file.set_permissions(permissions)?;
    }
    Ok(())
}
