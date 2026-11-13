//! A private, bounded reader for the file Git LFS appends progress to.
//!
//! The caller owns the process lifetime and must redact and authorize output
//! before sending it to a renderer. Git LFS itself receives only `path()`.
use anyhow::Result;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

const CHUNK_SIZE: usize = 8192;
const MAX_OUTPUT: u64 = 256 * 1024;

pub(crate) struct ProgressFile {
    directory: PathBuf,
    path: PathBuf,
    reader: Option<File>,
    consumed: u64,
}

impl ProgressFile {
    /// Create the directory atomically with private permissions before Git
    /// starts. Never use an inherited GIT_LFS_PROGRESS path.
    #[cfg(any(unix, windows))]
    pub(crate) fn create() -> Result<Self> {
        let directory = std::env::temp_dir()
            .canonicalize()?
            .join(format!("desktop-lfs-progress-{}", uuid::Uuid::new_v4()));
        create_private_directory(&directory)?;
        let path = directory.join("progress");
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path);
        match file {
            Ok(reader) => {
                #[cfg(windows)]
                if let Err(error) = verify_private_acl(&path, false) {
                    drop(reader);
                    let _ = std::fs::remove_file(&path);
                    let _ = std::fs::remove_dir(&directory);
                    return Err(error);
                }
                Ok(Self {
                    directory,
                    path,
                    reader: Some(reader),
                    consumed: 0,
                })
            }
            Err(error) => {
                let _ = std::fs::remove_dir(&directory);
                Err(error.into())
            }
        }
    }

    #[cfg(not(any(unix, windows)))]
    pub(crate) fn create() -> Result<Self> {
        anyhow::bail!("Private LFS progress files are not supported on this platform")
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Read appended data without following a replaced pathname. A fixed
    /// overall budget prevents an untrusted child from flooding the event stream.
    pub(crate) fn read_new(&mut self) -> std::io::Result<Vec<u8>> {
        let remaining = MAX_OUTPUT.saturating_sub(self.consumed) as usize;
        if remaining == 0 {
            return Ok(Vec::new());
        }
        let mut data = vec![0; CHUNK_SIZE.min(remaining)];
        let reader = self.reader.as_mut().expect("progress file is open");
        reader.seek(SeekFrom::Start(self.consumed))?;
        let count = reader.read(&mut data)?;
        data.truncate(count);
        self.consumed += count as u64;
        Ok(data)
    }
}

impl Drop for ProgressFile {
    fn drop(&mut self) {
        drop(self.reader.take());
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_dir(&self.directory);
    }
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(path)?;
    Ok(())
}

#[cfg(windows)]
fn create_private_directory(path: &Path) -> Result<()> {
    use std::{os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
            },
            SECURITY_ATTRIBUTES,
        },
        Storage::FileSystem::CreateDirectoryW,
    };

    // Explicitly bind access to the token user rather than OWNER RIGHTS,
    // since an elevated process can have a group as its default owner.
    let descriptor_text: Vec<u16> =
        format!("D:P(A;OICI;FA;;;{})(A;OICI;FA;;;SY)\0", token_user_sid()?)
            .encode_utf16()
            .collect();
    let mut descriptor = ptr::null_mut();
    // SAFETY: text is NUL terminated; descriptor points to writable storage.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            descriptor_text.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: name is NUL terminated and attributes and its descriptor are live.
    let created = unsafe { CreateDirectoryW(name.as_ptr(), &attributes) };
    let error = if created == 0 {
        Some(std::io::Error::last_os_error())
    } else {
        None
    };
    // SAFETY: descriptor is the LocalAlloc-owned result from the conversion call.
    unsafe { LocalFree(descriptor) };
    if let Some(error) = error {
        return Err(error.into());
    }
    if let Err(error) = verify_private_acl(path, true) {
        let _ = std::fs::remove_dir(path);
        return Err(error);
    }
    Ok(())
}

#[cfg(windows)]
fn verify_private_acl(path: &Path, protected: bool) -> Result<()> {
    use std::{os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{
            Authorization::{
                ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW,
                SDDL_REVISION_1, SE_FILE_OBJECT,
            },
            DACL_SECURITY_INFORMATION,
        },
    };
    let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut descriptor = ptr::null_mut();
    // SAFETY: name is terminated and descriptor points to writable storage.
    let status = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(std::io::Error::from_raw_os_error(status as i32).into());
    }
    let mut text = ptr::null_mut();
    let mut length = 0;
    // SAFETY: descriptor is live and both outputs point to writable storage.
    let converted = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            DACL_SECURITY_INFORMATION,
            &mut text,
            &mut length,
        )
    };
    let error = if converted == 0 {
        Some(std::io::Error::last_os_error())
    } else {
        None
    };
    // SAFETY: descriptor was allocated by GetNamedSecurityInfoW.
    unsafe { LocalFree(descriptor) };
    if let Some(error) = error {
        return Err(error.into());
    }
    // SAFETY: the API returned a UTF-16 allocation of length code units.
    let acl =
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length as usize) });
    // SAFETY: text was allocated by the descriptor conversion API.
    unsafe { LocalFree(text.cast()) };
    let user = token_user_sid()?;
    let user_sid = if user == "S-1-5-18" { "SY" } else { &user };
    anyhow::ensure!(
        (!protected || acl.starts_with("D:P"))
            && acl.matches("(A;").count() == 2
            && acl.matches('(').count() == 2
            && acl.contains(&format!(";FA;;;{user_sid})"))
            && acl.contains(";FA;;;SY)"),
        "LFS progress directory does not have a private ACL"
    );
    Ok(())
}

#[cfg(windows)]
fn token_user_sid() -> Result<String> {
    use std::{
        os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{
            Authorization::ConvertSidToStringSidW, GetTokenInformation, TOKEN_QUERY, TOKEN_USER,
            TokenUser,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    let mut handle = ptr::null_mut();
    // SAFETY: GetCurrentProcess returns a pseudo-handle and handle is writable.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: OpenProcessToken returned an owned, live handle.
    let token = unsafe { OwnedHandle::from_raw_handle(handle) };
    let mut length = 0u32;
    // SAFETY: the zero-length probe requests only the required size.
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            ptr::null_mut(),
            0,
            &mut length,
        )
    };
    anyhow::ensure!(
        length >= std::mem::size_of::<TOKEN_USER>() as u32 && length <= 65536,
        "Invalid token user information"
    );
    // usize allocation provides alignment for TOKEN_USER and its contained SID.
    let mut storage = vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
    // SAFETY: storage is aligned, writable, and at least length bytes long.
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            storage.as_mut_ptr().cast(),
            length,
            &mut length,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: the successful TokenUser query initialized a TOKEN_USER here.
    let user = unsafe { &*storage.as_ptr().cast::<TOKEN_USER>() };
    let mut sid = ptr::null_mut();
    // SAFETY: the token query returned a live SID and sid is writable.
    if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut count = 0;
    // SAFETY: the API returns a NUL-terminated SID string.
    while unsafe { *sid.add(count) } != 0 {
        count += 1;
    }
    // SAFETY: count is the length of the API-provided UTF-16 string.
    let result = String::from_utf16(unsafe { std::slice::from_raw_parts(sid, count) });
    // SAFETY: sid is the LocalAlloc-owned result from the conversion call.
    unsafe { LocalFree(sid.cast()) };
    Ok(result?)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{io::Write, os::unix::fs::PermissionsExt};

    #[test]
    fn owns_private_file_reads_appends_and_cleans_up() {
        let mut progress = ProgressFile::create().unwrap();
        let path = progress.path().to_owned();
        let directory = path.parent().unwrap().to_owned();
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(progress.read_new().unwrap().is_empty());
        let mut writer = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writer.write_all(b"download 1/2 4/8 example\n").unwrap();
        assert_eq!(progress.read_new().unwrap(), b"download 1/2 4/8 example\n");
        writer.write_all(b"download 1/2 8/8 example\n").unwrap();
        assert_eq!(progress.read_new().unwrap(), b"download 1/2 8/8 example\n");
        drop(writer);
        drop(progress);
        assert!(!directory.exists());
    }

    #[test]
    fn limits_progress() {
        let mut progress = ProgressFile::create().unwrap();
        let path = progress.path().to_owned();
        std::fs::write(&path, vec![b'x'; MAX_OUTPUT as usize + CHUNK_SIZE]).unwrap();
        let mut bytes = 0;
        loop {
            let chunk = progress.read_new().unwrap();
            if chunk.is_empty() {
                break;
            }
            bytes += chunk.len();
        }
        assert_eq!(bytes, MAX_OUTPUT as usize);
    }

    #[test]
    fn reads_original_handle_after_path_replaced() {
        let mut progress = ProgressFile::create().unwrap();
        let path = progress.path().to_owned();
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", &path).unwrap();
        assert!(progress.read_new().unwrap().is_empty());
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::{io::Write, os::windows::ffi::OsStrExt, ptr};

    fn dacl(path: &Path) -> String {
        use windows_sys::Win32::{
            Foundation::LocalFree,
            Security::{
                Authorization::{
                    ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW,
                    SDDL_REVISION_1, SE_FILE_OBJECT,
                },
                DACL_SECURITY_INFORMATION,
            },
        };
        let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut descriptor = ptr::null_mut();
        // SAFETY: name is terminated and all requested output pointers are valid.
        assert_eq!(
            unsafe {
                GetNamedSecurityInfoW(
                    name.as_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &mut descriptor,
                )
            },
            0
        );
        let mut text = ptr::null_mut();
        let mut length = 0;
        // SAFETY: descriptor is live and output pointers are writable.
        assert_ne!(
            unsafe {
                ConvertSecurityDescriptorToStringSecurityDescriptorW(
                    descriptor,
                    SDDL_REVISION_1,
                    DACL_SECURITY_INFORMATION,
                    &mut text,
                    &mut length,
                )
            },
            0
        );
        // SAFETY: text contains length UTF-16 units allocated by the API.
        let result =
            String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length as usize) });
        // SAFETY: both allocations are returned by Windows LocalAlloc APIs.
        unsafe {
            LocalFree(text.cast());
            LocalFree(descriptor);
        }
        result
    }

    #[test]
    fn rejects_unprotected_directory_and_cleans_up() {
        let path = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("desktop-lfs-acl-test-{}", uuid::Uuid::new_v4()));
        match create_private_directory(&path) {
            Ok(()) => {
                verify_private_acl(&path, true).unwrap();
                std::fs::remove_dir(path).unwrap();
            }
            Err(error) => {
                assert!(error.to_string().contains("private ACL"), "{error}");
                assert!(!path.exists());
            }
        }
    }

    #[test]
    fn private_progress_file_reads_and_cleans_up() {
        let mut progress = match ProgressFile::create() {
            Ok(progress) => progress,
            Err(error) if error.to_string().contains("private ACL") => return,
            Err(error) => panic!("could not create progress file: {error}"),
        };
        let path = progress.path().to_owned();
        let directory = path.parent().unwrap().to_owned();
        for target in [&directory, &path] {
            let acl = dacl(target);
            assert_eq!(acl.matches("(A;").count(), 2, "{acl}");
            assert!(
                acl.contains(&format!(";;;{})", token_user_sid().unwrap())),
                "{acl}"
            );
            assert!(
                acl.contains(";;;SY)") || acl.contains(";;;S-1-5-18)"),
                "{acl}"
            );
        }
        assert!(dacl(&directory).starts_with("D:P"));
        let mut writer = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writer.write_all(b"download 1/1 1/1 file\n").unwrap();
        assert_eq!(progress.read_new().unwrap(), b"download 1/1 1/1 file\n");
        drop(writer);
        drop(progress);
        assert!(!directory.exists());
    }
}
