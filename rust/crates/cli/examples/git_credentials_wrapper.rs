//! Isolated Git instrumentation for the offline credential integration tests.
//! It never reads Git's stdin or records arguments, credentials, or working paths.

#[cfg(unix)]
mod unix {
    use serde_json::{Value, json};
    use std::{
        env,
        ffi::{OsStr, OsString},
        fs::{File, OpenOptions},
        io::{self, Read, Write},
        os::unix::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawFd, process::CommandExt},
        path::PathBuf,
        process::Command,
        thread,
        time::{Duration, Instant},
    };

    const COMMANDS: &[&str] = &[
        "add",
        "apply",
        "archive",
        "bisect",
        "blame",
        "branch",
        "bundle",
        "cat-file",
        "checkout",
        "checkout-index",
        "cherry-pick",
        "clean",
        "clone",
        "commit",
        "config",
        "count-objects",
        "credential",
        "describe",
        "diff",
        "diff-files",
        "diff-index",
        "diff-tree",
        "fetch",
        "for-each-ref",
        "fsck",
        "gc",
        "hash-object",
        "help",
        "init",
        "lfs",
        "log",
        "ls-files",
        "ls-remote",
        "ls-tree",
        "merge",
        "merge-base",
        "merge-file",
        "mktag",
        "mktree",
        "mv",
        "notes",
        "pack-objects",
        "prune",
        "pull",
        "push",
        "read-tree",
        "rebase",
        "reflog",
        "remote",
        "reset",
        "restore",
        "rev-list",
        "rev-parse",
        "revert",
        "rm",
        "show",
        "show-ref",
        "sparse-checkout",
        "stash",
        "status",
        "stripspace",
        "submodule",
        "symbolic-ref",
        "tag",
        "update-index",
        "update-ref",
        "var",
        "version",
        "worktree",
        "write-tree",
    ];

    fn read_json(path: &OsStr) -> io::Result<Value> {
        let file = File::open(path)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::other("fixture file is not regular"));
        }
        let mut data = Vec::new();
        file.take(65_537).read_to_end(&mut data)?;
        if data.len() > 65_536 {
            return Err(io::Error::other("fixture file too large"));
        }
        serde_json::from_slice(&data).map_err(io::Error::other)
    }

    fn command_argument(args: &[OsString]) -> Option<&OsStr> {
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let bytes = arg.as_bytes();
            if [
                b"-C".as_slice(),
                b"-c",
                b"--git-dir",
                b"--work-tree",
                b"--namespace",
            ]
            .contains(&bytes)
            {
                args.next();
            } else if !bytes.starts_with(b"-") {
                return Some(arg);
            }
        }
        None
    }

    fn contains_secret(value: &OsStr, secrets: &[&str]) -> bool {
        let bytes = value.as_bytes();
        secrets.iter().filter(|s| !s.is_empty()).any(|secret| {
            bytes
                .windows(secret.len())
                .any(|part| part == secret.as_bytes())
        })
    }

    fn log_call(command: Option<&OsStr>, args: &[OsString], secrets: &[&str]) -> io::Result<()> {
        let logged = command.map(|arg| {
            arg.to_str()
                .filter(|s| COMMANDS.contains(s) && !contains_secret(arg, secrets))
                .unwrap_or("other")
        });
        let clean = !args.iter().any(|arg| contains_secret(arg, secrets));
        let alice_clean =
            !env::vars_os().any(|(_, value)| value.as_bytes() == secrets[0].as_bytes());
        let mut line = serde_json::to_vec(
            &json!({"command":logged,"argv_clean":clean,"alice_environment_clean":alice_clean}),
        )
        .map_err(io::Error::other)?;
        line.push(b'\n');
        let path =
            env::var_os("CREDENTIALS_GIT_LOG").ok_or_else(|| io::Error::other("missing log"))?;
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(path)?;
        // This file is shared by concurrent fixture processes. The fd remains open
        // throughout the append and closing it releases the advisory lock.
        loop {
            // SAFETY: the descriptor remains open for the duration of the call.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
                break;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
        file.write_all(&line)
    }

    fn wait_for_gate(command: Option<&OsStr>) -> io::Result<bool> {
        let path = env::var_os("CREDENTIALS_CONTROL")
            .ok_or_else(|| io::Error::other("missing control"))?;
        let control = read_json(&path)?;
        let gate = control
            .as_object()
            .ok_or_else(|| io::Error::other("invalid control"))?
            .get("gate")
            .and_then(Value::as_str);
        let inherited = env::var_os("CREDENTIALS_GATE_CHILD").is_some_and(|v| !v.is_empty());
        if inherited
            || !gate.is_some_and(|gate| {
                !gate.is_empty() && gate.len() < 128 && command == Some(OsStr::new(gate))
            })
        {
            return Ok(false);
        }
        let started = env::var_os("CREDENTIALS_GATE_STARTED")
            .ok_or_else(|| io::Error::other("missing gate"))?;
        let release = PathBuf::from(
            env::var_os("CREDENTIALS_GATE_RELEASE")
                .ok_or_else(|| io::Error::other("missing gate"))?,
        );
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(started)?;
        file.write_all(b"started")?;
        let deadline = Instant::now() + Duration::from_secs(25);
        while !release.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        Ok(true)
    }

    pub fn run() -> io::Result<()> {
        // SAFETY: restoring these signals to their defaults does not install a handler.
        // This also ensures the subsequently executed Git inherits normal behavior.
        unsafe {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGTERM, libc::SIG_DFL);
            libc::signal(libc::SIGHUP, libc::SIG_DFL);
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        }
        let config = env::var_os("CREDENTIALS_WRAPPER_CONFIG")
            .ok_or_else(|| io::Error::other("missing fixture config"))?;
        let config = read_json(&config)?;
        let get = |key| {
            config
                .get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| io::Error::other("invalid fixture config"))
        };
        let real_git = get("real_git")?;
        let secrets = [get("alice")?, get("bob")?, get("environment")?];
        let args: Vec<_> = env::args_os().skip(1).collect();
        let command = command_argument(&args);
        log_call(command, &args, &secrets)?;
        let gated = wait_for_gate(command)?;
        let mut process = Command::new(real_git);
        process.args(args);
        if gated {
            process.env("CREDENTIALS_GATE_CHILD", "1");
        }
        let _ = process.exec();
        eprintln!("Git credential fixture instrumentation failed");
        std::process::exit(127);
    }
}

fn main() {
    #[cfg(unix)]
    if unix::run().is_err() {
        eprintln!("Git credential fixture instrumentation failed");
        std::process::exit(125);
    }
    #[cfg(not(unix))]
    {
        eprintln!("Git credential fixture instrumentation requires Unix");
        std::process::exit(125);
    }
}
