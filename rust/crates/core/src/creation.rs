//! Writes the fixed set of files offered by the new-repository dialog.
use crate::{Repository, cancellation};
use anyhow::{Result, ensure};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use serde::Deserialize;
use serde_json::Value;
use std::{io::Write, path::Path};

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreationFile {
    Readme,
    Gitignore,
    License,
    GitAttributes,
}

impl CreationFile {
    fn name(self) -> &'static str {
        match self {
            Self::Readme => "README.md",
            Self::Gitignore => ".gitignore",
            Self::License => "LICENSE",
            Self::GitAttributes => ".gitattributes",
        }
    }
}

impl Repository {
    pub(crate) async fn write_creation_file(
        &self,
        kind: CreationFile,
        contents: String,
    ) -> Result<Value> {
        ensure!(contents.len() <= 65536, "Creation file exceeds 64 KiB");
        let root = self.path.clone();
        cancellation::blocking(move || write_file(&root, kind, contents.as_bytes())).await?;
        Ok(Value::Null)
    }
}

pub(crate) fn write_file(root: &Path, kind: CreationFile, contents: &[u8]) -> Result<()> {
    let directory = Dir::open_ambient_dir(root, ambient_authority())?;
    let name = kind.name();
    let preserve_existing = matches!(kind, CreationFile::GitAttributes);
    let permissions = if preserve_existing {
        None
    } else {
        let mut options = OpenOptions::new();
        options.write(true).follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        match directory.open_with(name, &options) {
            Ok(file) => {
                let metadata = file.metadata()?;
                ensure!(metadata.is_file(), "Creation file is not a regular file");
                Some(metadata.permissions())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        }
    };
    let temporary = format!(".desktop-create-{}", uuid::Uuid::new_v4());
    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o666);
        }
        let mut file = directory.open_with(&temporary, &options)?;
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)?;
        }
        file.write_all(contents)?;
        file.sync_all()?;
        cancellation::Cancellation::current().check()?;
        if preserve_existing {
            match directory.hard_link(&temporary, &directory, name) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
            directory.remove_file(&temporary)?;
        } else {
            match directory.symlink_metadata(name) {
                Ok(metadata) => ensure!(metadata.is_file(), "Creation file is not a regular file"),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            directory.rename(&temporary, &directory, name)?;
        }
        #[cfg(unix)]
        directory.open(".")?.into_std().sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = directory.remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_only_known_files_and_preserves_existing_attributes() {
        let dir = std::env::temp_dir().join(format!("desktop-creation-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        write_file(&dir, CreationFile::Readme, b"first").unwrap();
        write_file(&dir, CreationFile::Readme, "second 🦀".as_bytes()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("README.md")).unwrap(),
            "second 🦀"
        );
        write_file(&dir, CreationFile::GitAttributes, b"original").unwrap();
        write_file(&dir, CreationFile::GitAttributes, b"replacement").unwrap();
        assert_eq!(
            std::fs::read(dir.join(".gitattributes")).unwrap(),
            b"original"
        );
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn refuses_to_follow_symlinks() {
        use std::os::unix::fs::symlink;
        let dir = std::env::temp_dir().join(format!("desktop-creation-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let outside = dir.join("outside");
        std::fs::write(&outside, b"untouched").unwrap();
        symlink(&outside, dir.join("LICENSE")).unwrap();
        assert!(write_file(&dir, CreationFile::License, b"bad").is_err());
        assert_eq!(std::fs::read(&outside).unwrap(), b"untouched");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
