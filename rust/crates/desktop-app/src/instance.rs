//! Single-instance handoff.
//!
//! The OAuth callback and "Open in Desktop" links arrive as a *new* process
//! launched by the OS with the URL as an argument. That process must hand the
//! URL to the window that is already running (it holds the pending sign-in)
//! and exit, as Electron's `requestSingleInstanceLock` did.

use std::path::Path;

/// Arguments worth forwarding to a running instance.
pub fn is_forwardable(arg: &str) -> bool {
    desktop_core::app::is_app_url(arg)
        || arg.starts_with("--cli-open=")
        || arg.starts_with("--cli-clone=")
}

#[cfg(target_os = "linux")]
mod imp {
    use std::fs::{File, OpenOptions};
    use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use std::time::Duration;

    const LOCK_MAGIC: &str = "github-desktop-rs-instance-v1";

    fn refused(message: &'static str) -> io::Error {
        io::Error::new(io::ErrorKind::PermissionDenied, message)
    }

    fn private_parent(socket: &Path) -> io::Result<&Path> {
        let parent = socket
            .parent()
            .filter(|value| !value.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let metadata = std::fs::symlink_metadata(parent)?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o022 != 0
        {
            return Err(refused("instance directory is not privately owned"));
        }
        Ok(parent)
    }

    fn lock_path(socket: &Path) -> PathBuf {
        let mut name = socket.as_os_str().to_os_string();
        name.push(".lock");
        PathBuf::from(name)
    }

    fn read_marker(file: &mut File) -> io::Result<(String, u64)> {
        file.seek(SeekFrom::Start(0))?;
        let mut contents = String::new();
        (&*file)
            .take(16 * 1024 * 1024 + 1)
            .read_to_string(&mut contents)?;
        if contents.len() > 16 * 1024 * 1024 {
            return Err(refused("instance lock is unexpectedly large"));
        }
        let Some(complete) = contents.rsplit_once('\n').map(|(value, _)| value) else {
            return Err(refused("instance lock is incomplete"));
        };
        let mut lines = complete.lines();
        if lines.next() != Some(LOCK_MAGIC) {
            return Err(refused("instance lock does not belong to this application"));
        }
        let mut marker = String::new();
        for line in lines {
            let Some((device, inode)) = line.split_once(':') else {
                return Err(refused("invalid instance marker"));
            };
            if device.parse::<u64>().is_err() || inode.parse::<u64>().is_err() {
                return Err(refused("invalid instance marker"));
            }
            marker = line.to_owned();
        }
        Ok((marker, (complete.len() + 1) as u64))
    }

    /// Accept only a socket recorded in a private application lock file.
    fn socket_identity(socket: &Path) -> io::Result<(u64, u64)> {
        private_parent(socket)?;
        let uid = unsafe { libc::geteuid() };
        let socket_info = std::fs::symlink_metadata(socket)?;
        if !socket_info.file_type().is_socket() || socket_info.uid() != uid {
            return Err(refused("instance path is not an owned socket"));
        }
        let path = lock_path(socket);
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)?;
        let info = file.metadata()?;
        let entry = std::fs::symlink_metadata(path)?;
        if !info.is_file()
            || info.uid() != uid
            || info.mode() & 0o077 != 0
            || info.nlink() != 1
            || info.dev() != entry.dev()
            || info.ino() != entry.ino()
        {
            return Err(refused("instance lock is not a private owned file"));
        }
        let (marker, _) = read_marker(&mut file)?;
        let identity = (socket_info.dev(), socket_info.ino());
        if marker != format!("{}:{}", identity.0, identity.1) {
            return Err(refused("instance socket does not match its marker"));
        }
        Ok(identity)
    }

    /// The lock file is permanent: removing a locked inode would permit a
    /// second process to create and lock another file at the same path.
    fn acquire_lock(socket: &Path) -> io::Result<(File, String)> {
        let parent = private_parent(socket)?;
        let uid = unsafe { libc::geteuid() };
        let lock_path = lock_path(socket);
        let options = || {
            let mut options = OpenOptions::new();
            options
                .read(true)
                .write(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
            options
        };
        // Initialize and lock a temporary inode before publishing the lock
        // name. A competing process cannot observe a half-written marker.
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let temporary = parent.join(format!(".l{}", &nonce[..8]));
        let mut created_file = options().create_new(true).open(&temporary)?;
        let publication = (|| {
            lock_exclusive(&created_file)?;
            writeln!(created_file, "{LOCK_MAGIC}")?;
            created_file.sync_data()?;
            std::fs::hard_link(&temporary, &lock_path)
        })();
        let _ = std::fs::remove_file(&temporary);
        let mut file = match publication {
            Ok(()) => created_file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                drop(created_file);
                let existing = options().open(&lock_path)?;
                lock_exclusive(&existing)?;
                existing
            }
            Err(error) => return Err(error),
        };
        let metadata = file.metadata()?;
        let entry = std::fs::symlink_metadata(&lock_path)?;
        if !metadata.is_file()
            || metadata.uid() != uid
            || metadata.mode() & 0o077 != 0
            || metadata.nlink() > 2
            || metadata.dev() != entry.dev()
            || metadata.ino() != entry.ino()
        {
            return Err(refused("instance lock is not a private owned file"));
        }
        let (marker, complete_len) = read_marker(&mut file)?;
        if metadata.nlink() == 2 {
            // A crash may occur after publishing the lock but before removing
            // its private temporary name. Remove only our exact staging name
            // with the same inode, then verify that no other link remains.
            let mut removed = false;
            for candidate in std::fs::read_dir(parent)? {
                let candidate = candidate?;
                let name = candidate.file_name();
                let Some(name) = name.to_str() else { continue };
                if name.len() != 10
                    || !name.starts_with(".l")
                    || !name[2..].bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    continue;
                }
                let info = std::fs::symlink_metadata(candidate.path())?;
                if info.is_file()
                    && info.uid() == uid
                    && info.dev() == metadata.dev()
                    && info.ino() == metadata.ino()
                {
                    std::fs::remove_file(candidate.path())?;
                    removed = true;
                    break;
                }
            }
            if !removed || file.metadata()?.nlink() != 1 {
                return Err(refused("instance lock has an unexpected hard link"));
            }
        }
        if metadata.len() != complete_len {
            file.set_len(complete_len)?;
        }
        Ok((file, marker))
    }

    fn lock_exclusive(file: &File) -> io::Result<()> {
        // SAFETY: file owns a valid descriptor; flock does not retain pointers.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = io::Error::last_os_error();
            return Err(if error.kind() == io::ErrorKind::WouldBlock {
                io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "another application instance owns the socket",
                )
            } else {
                error
            });
        }
        Ok(())
    }

    fn write_marker(file: &mut File, identity: (u64, u64)) -> io::Result<()> {
        // Append and sync rather than truncate: a crash partway through the
        // next record cannot destroy the last complete socket identity.
        file.seek(SeekFrom::End(0))?;
        writeln!(file, "{}:{}", identity.0, identity.1)?;
        file.sync_data()
    }

    /// A full listener backlog can block connect(2). Never block the caller;
    /// a timed-out worker drops any connection it eventually establishes.
    fn connect_with_timeout(socket: &Path) -> io::Result<UnixStream> {
        let path = socket.to_owned();
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("desktop-instance-probe".into())
            .spawn(move || {
                let _ = sender.send(UnixStream::connect(path));
            })?;
        receiver
            .recv_timeout(Duration::from_millis(500))
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "instance socket connection timed out",
                )
            })?
    }

    /// Timeout or ambiguous errors never authorize removing a socket.
    fn listener_is_stale(socket: &Path) -> io::Result<bool> {
        match connect_with_timeout(socket) {
            Ok(_) => Ok(false),
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => Ok(true),
            Err(error) => Err(error),
        }
    }

    fn create_listener(socket: &Path, lock: &mut File, marker: &str) -> io::Result<UnixListener> {
        match std::fs::symlink_metadata(socket) {
            Ok(metadata) => {
                let uid = unsafe { libc::geteuid() };
                if !metadata.file_type().is_socket() || metadata.uid() != uid {
                    return Err(refused("instance path is not an owned socket"));
                }
                if !listener_is_stale(socket)? {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "another process is listening on the instance socket",
                    ));
                }
                if marker != format!("{}:{}", metadata.dev(), metadata.ino()) {
                    return Err(refused("refusing to remove an unrecognized socket"));
                }
                let current = std::fs::symlink_metadata(socket)?;
                if current.dev() != metadata.dev() || current.ino() != metadata.ino() {
                    return Err(refused("instance socket changed during review"));
                }
                std::fs::remove_file(socket)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let parent = socket
            .parent()
            .filter(|value| !value.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        // Publish by hard link only after recording the inode. A crash at any
        // later point leaves an identifiable socket that the next owner can
        // recover; hard_link also refuses to replace another process's path.
        // Keep the temporary basename no longer than `instance.sock` so that
        // a target near the platform's Unix-socket path limit still works.
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let staging = parent.join(format!(".s{}", &nonce[..8]));
        let listener = UnixListener::bind(&staging)?;
        let result: io::Result<()> = (|| {
            std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o600))?;
            let metadata = std::fs::symlink_metadata(&staging)?;
            write_marker(lock, (metadata.dev(), metadata.ino()))?;
            std::fs::hard_link(&staging, socket)?;
            Ok(())
        })();
        let _ = std::fs::remove_file(&staging);
        result?;
        Ok(listener)
    }

    /// Hand `args` only to a marked instance. Older unmarked instances must be
    /// restarted before they can receive callbacks from this version.
    pub fn forward(socket: &Path, args: &[String]) -> bool {
        let Ok(identity) = socket_identity(socket) else {
            return false;
        };
        let Ok(mut stream) = connect_with_timeout(socket) else {
            return false;
        };
        if socket_identity(socket).ok() != Some(identity) {
            return false;
        }
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let Ok(mut line) = serde_json::to_string(args) else {
            return false;
        };
        line.push('\n');
        if stream.write_all(line.as_bytes()).is_err() {
            return false;
        }
        // Wait for the acknowledgement so we only exit once it was received.
        let mut ack = String::new();
        BufReader::new(stream).read_line(&mut ack).is_ok() && ack.trim() == "ok"
    }

    /// Become the primary instance and call `on_args` for each handoff.
    pub fn listen(
        socket: &Path,
        on_args: impl Fn(Vec<String>) + Send + 'static,
    ) -> std::io::Result<()> {
        let (mut lock, marker) = acquire_lock(socket)?;
        let listener = create_listener(socket, &mut lock, &marker)?;

        std::thread::Builder::new()
            .name("desktop-instance".into())
            .spawn(move || {
                // The lock lives for as long as this listener owns the path.
                let _lock = lock;
                for stream in listener.incoming().flatten() {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                    let mut line = String::new();
                    if BufReader::new(&stream).read_line(&mut line).is_err() {
                        continue;
                    }
                    match serde_json::from_str::<Vec<String>>(line.trim()) {
                        Ok(args) => {
                            let _ = (&stream).write_all(b"ok\n");
                            on_args(args);
                        }
                        Err(error) => tracing::warn!(%error, "ignoring malformed instance handoff"),
                    }
                }
            })?;
        Ok(())
    }
}

// Keep the existing macOS/other Unix transport until the Linux ownership
// protocol is exercised on those filesystems and WebViews as well.
#[cfg(all(unix, not(target_os = "linux")))]
mod imp {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;
    use std::time::Duration;

    pub fn forward(socket: &Path, args: &[String]) -> bool {
        let Ok(mut stream) = UnixStream::connect(socket) else {
            return false;
        };
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let Ok(mut line) = serde_json::to_string(args) else {
            return false;
        };
        line.push('\n');
        if stream.write_all(line.as_bytes()).is_err() {
            return false;
        }
        let mut ack = String::new();
        BufReader::new(stream).read_line(&mut ack).is_ok() && ack.trim() == "ok"
    }

    pub fn listen(
        socket: &Path,
        on_args: impl Fn(Vec<String>) + Send + 'static,
    ) -> std::io::Result<()> {
        let _ = std::fs::remove_file(socket);
        let listener = UnixListener::bind(socket)?;
        std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
        std::thread::Builder::new()
            .name("desktop-instance".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                    let mut line = String::new();
                    if BufReader::new(&stream).read_line(&mut line).is_err() {
                        continue;
                    }
                    match serde_json::from_str::<Vec<String>>(line.trim()) {
                        Ok(args) => {
                            let _ = (&stream).write_all(b"ok\n");
                            on_args(args);
                        }
                        Err(error) => tracing::warn!(%error, "ignoring malformed instance handoff"),
                    }
                }
            })?;
        Ok(())
    }
}

#[cfg(not(unix))]
mod imp {
    use std::path::Path;

    // Not implemented for this platform yet: every launch opens its own window.
    pub fn forward(_socket: &Path, _args: &[String]) -> bool {
        false
    }
    pub fn listen(
        _socket: &Path,
        _on_args: impl Fn(Vec<String>) + Send + 'static,
    ) -> std::io::Result<()> {
        Ok(())
    }
}

pub fn forward(socket: &Path, args: &[String]) -> bool {
    imp::forward(socket, args)
}

pub fn listen(
    socket: &Path,
    on_args: impl Fn(Vec<String>) + Send + 'static,
) -> std::io::Result<()> {
    imp::listen(socket, on_args)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc;
    use std::sync::{Arc, Barrier};
    use std::time::Duration;

    fn test_directory() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("desktop-instance-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        dir
    }

    struct ProcessGuard(Child);
    impl Drop for ProcessGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn a_second_launch_hands_its_arguments_to_the_first() {
        let directory = test_directory();
        let socket = directory.join("instance.sock");
        let args = vec!["x-github-desktop-dev-auth://oauth?code=c&state=s".to_string()];

        assert!(!forward(&socket, &args), "nothing is listening yet");

        let (sender, receiver) = mpsc::channel();
        listen(&socket, move |received| sender.send(received).unwrap()).unwrap();

        assert!(forward(&socket, &args));
        assert_eq!(receiver.recv_timeout(Duration::from_secs(5)).unwrap(), args);
        let error = listen(&socket, |_| {}).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
        assert!(forward(&socket, &args));
        assert_eq!(receiver.recv_timeout(Duration::from_secs(5)).unwrap(), args);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn concurrent_launches_leave_exactly_one_owner() {
        let directory = test_directory();
        let socket = directory.join("instance.sock");
        let barrier = Arc::new(Barrier::new(8));
        let (sender, receiver) = mpsc::channel();
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let socket = socket.clone();
                let barrier = barrier.clone();
                let sender = sender.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    listen(&socket, move |args| {
                        let _ = sender.send(args);
                    })
                    .is_ok()
                })
            })
            .collect();
        assert_eq!(
            threads
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .filter(|won| *won)
                .count(),
            1
        );
        let args = vec!["x-github-desktop-dev-auth://oauth?code=c&state=s".to_owned()];
        assert!(forward(&socket, &args));
        assert_eq!(receiver.recv_timeout(Duration::from_secs(5)).unwrap(), args);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn foreign_paths_and_unrecognized_sockets_are_never_removed() {
        let directory = test_directory();
        let file = directory.join("file.sock");
        std::fs::write(&file, b"keep").unwrap();
        assert!(listen(&file, |_| {}).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"keep");

        let target = directory.join("target");
        let link = directory.join("symlink.sock");
        std::fs::write(&target, b"target").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(listen(&link, |_| {}).is_err());
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"target");

        let socket = directory.join("foreign.sock");
        let foreign = UnixListener::bind(&socket).unwrap();
        foreign.set_nonblocking(true).unwrap();
        let args = vec!["x-github-desktop-dev-auth://oauth?code=secret&state=s".to_owned()];
        assert!(
            !forward(&socket, &args),
            "an unmarked listener must not receive OAuth data"
        );
        assert_eq!(
            foreign.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        let foreign_lock = directory.join("foreign.sock.lock");
        std::fs::write(&foreign_lock, b"github-desktop-rs-instance-v1\n0:0\n").unwrap();
        std::fs::set_permissions(
            &foreign_lock,
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        )
        .unwrap();
        assert!(
            !forward(&socket, &args),
            "a mismatched marker must not receive OAuth data"
        );
        assert_eq!(
            foreign.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert!(listen(&socket, |_| {}).is_err());
        assert!(UnixStream::connect(&socket).is_ok());
        drop(foreign);
        // An inactive foreign socket has no matching application marker.
        assert!(listen(&socket, |_| {}).is_err());
        assert!(std::fs::symlink_metadata(&socket).is_ok());

        let lock_socket = directory.join("locked.sock");
        let unrelated_lock = directory.join("locked.sock.lock");
        std::fs::write(&unrelated_lock, b"leave this file alone").unwrap();
        std::fs::set_permissions(
            &unrelated_lock,
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        )
        .unwrap();
        assert!(listen(&lock_socket, |_| {}).is_err());
        assert_eq!(
            std::fs::read(&unrelated_lock).unwrap(),
            b"leave this file alone"
        );
        let symlink_socket = directory.join("symlink-lock.sock");
        let symlink_lock = directory.join("symlink-lock.sock.lock");
        std::os::unix::fs::symlink(&target, &symlink_lock).unwrap();
        assert!(listen(&symlink_socket, |_| {}).is_err());
        assert!(
            std::fs::symlink_metadata(&symlink_lock)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"target");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn forwarding_times_out_when_listener_backlog_is_full() {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let directory = test_directory();
        let socket = directory.join("full.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        // SAFETY: the listener owns a valid Unix socket descriptor.
        assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 1) }, 0);
        let metadata = std::fs::symlink_metadata(&socket).unwrap();
        let lock = directory.join("full.sock.lock");
        std::fs::write(
            &lock,
            format!(
                "github-desktop-rs-instance-v1\n{}:{}\n",
                metadata.dev(),
                metadata.ino()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut connections = Vec::new();
        let mut full = false;
        for _ in 0..1024 {
            let (tx, rx) = mpsc::sync_channel(1);
            let path = socket.clone();
            std::thread::spawn(move || {
                let _ = tx.send(UnixStream::connect(path));
            });
            match rx.recv_timeout(Duration::from_millis(150)) {
                Ok(Ok(stream)) => connections.push(stream),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    full = true;
                    break;
                }
                other => panic!("unexpected connection result: {other:?}"),
            }
        }
        assert!(full, "test did not saturate the socket backlog");
        let start = std::time::Instant::now();
        assert!(!forward(
            &socket,
            &["x-github-desktop-dev-auth://oauth?code=secret".into()]
        ));
        assert!(start.elapsed() < Duration::from_secs(3));
        drop(listener);
        drop(connections);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn child_listener_for_crash_test() {
        let Some(socket) = std::env::var_os("DESKTOP_INSTANCE_TEST_SOCKET") else {
            return;
        };
        if let Some(barrier) = std::env::var_os("DESKTOP_INSTANCE_TEST_BARRIER") {
            println!("INSTANCE_CHILD_READY");
            while !PathBuf::from(&barrier).exists() {
                std::thread::sleep(Duration::from_millis(2));
            }
            if listen(PathBuf::from(socket).as_path(), |_| {}).is_ok() {
                println!("INSTANCE_CHILD_WON");
                loop {
                    std::thread::park();
                }
            }
            println!("INSTANCE_CHILD_LOST");
            return;
        }
        listen(PathBuf::from(socket).as_path(), |_| {}).unwrap();
        println!("INSTANCE_CHILD_READY");
        loop {
            std::thread::park();
        }
    }

    #[test]
    fn simultaneous_process_launches_have_one_listener() {
        let directory = test_directory();
        let socket = directory.join("instance.sock");
        let barrier = directory.join("start");
        let (tx, rx) = mpsc::channel();
        let mut children = Vec::new();
        for _ in 0..8 {
            let mut child = ProcessGuard(
                Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "instance::tests::child_listener_for_crash_test",
                        "--nocapture",
                    ])
                    .env("DESKTOP_INSTANCE_TEST_SOCKET", &socket)
                    .env("DESKTOP_INSTANCE_TEST_BARRIER", &barrier)
                    .stdout(Stdio::piped())
                    .spawn()
                    .unwrap(),
            );
            let stdout = child.0.stdout.take().unwrap();
            let sender = tx.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if line.contains("INSTANCE_CHILD_READY") {
                        let _ = sender.send("ready");
                    } else if line.contains("INSTANCE_CHILD_WON") {
                        let _ = sender.send("won");
                    } else if line.contains("INSTANCE_CHILD_LOST") {
                        let _ = sender.send("lost");
                    }
                }
            });
            children.push(child);
        }
        for _ in 0..8 {
            assert_eq!(rx.recv_timeout(Duration::from_secs(15)).unwrap(), "ready");
        }
        std::fs::write(&barrier, b"go").unwrap();
        let mut winners = 0;
        for _ in 0..8 {
            if rx.recv_timeout(Duration::from_secs(15)).unwrap() == "won" {
                winners += 1;
            }
        }
        assert_eq!(winners, 1);
        assert!(forward(
            &socket,
            &["x-github-desktop-dev-auth://oauth?code=c&state=s".into()]
        ));
        drop(children);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recovers_its_socket_after_owner_process_is_killed() {
        let directory = test_directory();
        let socket = directory.join("instance.sock");
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "instance::tests::child_listener_for_crash_test",
                "--nocapture",
            ])
            .env("DESKTOP_INSTANCE_TEST_SOCKET", &socket)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut child = ProcessGuard(child);
        let stdout = child.0.stdout.take().unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line.contains("INSTANCE_CHILD_READY") {
                    let _ = ready_tx.send(());
                    break;
                }
            }
        });
        ready_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(listen(&socket, |_| {}).is_err());
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        // Simulate a crash partway through a subsequent journal record.
        let lock = directory.join("instance.sock.lock");
        use std::io::Write as _;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&lock)
            .unwrap()
            .write_all(b"123:")
            .unwrap();
        let interrupted_staging = directory.join(".l1234abcd");
        std::fs::hard_link(&lock, &interrupted_staging).unwrap();
        let (sender, receiver) = mpsc::channel();
        listen(&socket, move |args| {
            let _ = sender.send(args);
        })
        .unwrap();
        assert!(!interrupted_staging.exists());
        let args = vec!["--cli-open=/tmp/repository".to_owned()];
        assert!(forward(&socket, &args));
        assert_eq!(receiver.recv_timeout(Duration::from_secs(5)).unwrap(), args);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn only_urls_and_cli_actions_are_forwarded() {
        assert!(is_forwardable(
            "x-github-desktop-dev-auth://oauth?code=c&state=s"
        ));
        assert!(is_forwardable("--cli-open=/tmp/repo"));
        assert!(!is_forwardable("--self-test"));
        assert!(!is_forwardable("https://example.com"));
    }
}
