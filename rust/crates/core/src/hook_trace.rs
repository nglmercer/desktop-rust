//! Read Git Trace2 through a bounded pipe and expose only hook names/statuses.
use crate::hook_progress::HookProgress;
use anyhow::{Context, Result};
use std::{
    collections::HashMap,
    ffi::CString,
    fs,
    io::{ErrorKind, Read},
    os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    path::{Path, PathBuf},
};
use tokio::{
    io::unix::AsyncFd,
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

const MAX_LINE: usize = 64 * 1024;

struct Decoder {
    root: Option<String>,
    partial: Vec<u8>,
    overflow: bool,
    children: HashMap<u64, String>,
    checkpoint: String,
    sender: mpsc::Sender<(u64, HookProgress)>,
    request_id: u64,
    emitted: usize,
}

impl Decoder {
    fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if byte == b'\n' {
                if !self.overflow {
                    let line = std::mem::take(&mut self.partial);
                    self.line(&line);
                }
                self.partial.clear();
                self.overflow = false;
            } else if !self.overflow {
                if self.partial.len() == MAX_LINE {
                    self.partial.clear();
                    self.overflow = true;
                } else {
                    self.partial.push(byte);
                }
            }
        }
    }

    fn line(&mut self, bytes: &[u8]) {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
            return;
        };
        let Some(sid) = value["sid"].as_str() else {
            return;
        };
        if self.root.is_none() && value["event"] == "version" {
            self.root = Some(sid.to_owned());
        }
        if self.root.as_deref() != Some(sid) {
            return;
        }
        match value["event"].as_str() {
            Some("child_start") if value["child_class"] == "hook" => {
                let (Some(id), Some(name)) =
                    (value["child_id"].as_u64(), value["hook_name"].as_str())
                else {
                    return;
                };
                if !matches!(
                    name,
                    "pre-commit"
                        | "prepare-commit-msg"
                        | "commit-msg"
                        | "post-commit"
                        | "post-rewrite"
                        | "pre-auto-gc"
                ) || self.children.len() >= 64
                {
                    return;
                }
                self.children.insert(id, name.to_owned());
                self.emit(name, "started");
            }
            Some("child_exit") => {
                let (Some(id), Some(code)) = (value["child_id"].as_u64(), value["code"].as_i64())
                else {
                    return;
                };
                if let Some(name) = self.children.remove(&id) {
                    self.emit(&name, if code == 0 { "finished" } else { "failed" });
                }
            }
            _ => {}
        }
    }

    fn emit(&mut self, name: &str, status: &'static str) {
        if self.emitted >= 256 {
            return;
        }
        self.emitted += 1;
        let _ = self.sender.try_send((
            self.request_id,
            HookProgress {
                checkpoint_id: self.checkpoint.clone(),
                hook_name: name.to_owned(),
                status,
            },
        ));
    }
}

pub(crate) struct Trace {
    path: PathBuf,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl Trace {
    pub(crate) fn start(
        directory: &Path,
        checkpoint: &str,
        sender: mpsc::Sender<(u64, HookProgress)>,
        request_id: u64,
    ) -> Result<Self> {
        let path = directory.join(format!("hook-trace-{}", uuid::Uuid::new_v4()));
        let name = CString::new(path.as_os_str().as_bytes()).context("invalid trace pipe path")?;
        // The path is in the private checkpoint directory; O_EXCL semantics of
        // mkfifo prevent replacing an existing entry.
        if unsafe { libc::mkfifo(name.as_ptr(), 0o600) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let result = (|| -> Result<Self> {
            let file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
                .open(&path)?;
            // `file` owns a stable descriptor until the AsyncFd is dropped.
            let fd =
                unsafe { AsyncFd::register_with_interest(file, tokio::io::Interest::READABLE) }?;
            let (stop, mut stopped) = oneshot::channel();
            let mut decoder = Decoder {
                root: None,
                partial: Vec::new(),
                overflow: false,
                children: HashMap::new(),
                checkpoint: checkpoint.to_owned(),
                sender,
                request_id,
                emitted: 0,
            };
            let task = tokio::spawn(async move {
                let mut buffer = [0u8; 8192];
                loop {
                    tokio::select! {
                        _ = &mut stopped => {
                            // A detached hook may keep writing after Git exits;
                            // do not wait indefinitely for an empty pipe.
                            for _ in 0..256 {
                                match (&*fd.get_ref()).read(&mut buffer) {
                                    Ok(0) => break,
                                    Ok(count) => decoder.feed(&buffer[..count]),
                                    Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                                    Err(_) => break,
                                }
                            }
                            break;
                        }
                        ready = fd.readable() => {
                            let Ok(mut ready) = ready else { break; };
                            match ready.try_io(|fd| (&*fd.get_ref()).read(&mut buffer)) {
                                Ok(Ok(0)) => break,
                                Ok(Ok(count)) => decoder.feed(&buffer[..count]),
                                Ok(Err(_)) => break,
                                Err(_) => {},
                            }
                        }
                    }
                }
            });
            Ok(Self {
                path: path.clone(),
                stop: Some(stop),
                task: Some(task),
            })
        })();
        if result.is_err() {
            let _ = fs::remove_file(&path);
        }
        result
    }
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
    pub(crate) async fn finish(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}
impl Drop for Trace {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_only_owned_hook_names_and_statuses_from_fragmented_trace() {
        let (sender, mut receiver) = mpsc::channel(8);
        let mut decoder = Decoder {
            root: None,
            partial: Vec::new(),
            overflow: false,
            children: HashMap::new(),
            checkpoint: "checkpoint".into(),
            sender,
            request_id: 7,
            emitted: 0,
        };
        let trace = br#"{"event":"version","sid":"root","argv":["private secret"]}
{"event":"child_start","sid":"root/child","child_class":"hook","child_id":1,"hook_name":"pre-commit"}
{"event":"child_start","sid":"root","child_class":"hook","child_id":2,"hook_name":"reference-transaction"}
{"event":"child_start","sid":"root","child_class":"hook","child_id":3,"hook_name":"pre-commit","argv":["private secret"]}
{"event":"child_exit","sid":"root","child_id":3,"code":1}
"#;
        for chunk in trace.chunks(13) {
            decoder.feed(chunk);
        }
        assert_eq!(
            receiver.try_recv().unwrap(),
            (
                7,
                HookProgress {
                    checkpoint_id: "checkpoint".into(),
                    hook_name: "pre-commit".into(),
                    status: "started"
                }
            )
        );
        assert_eq!(
            receiver.try_recv().unwrap(),
            (
                7,
                HookProgress {
                    checkpoint_id: "checkpoint".into(),
                    hook_name: "pre-commit".into(),
                    status: "failed"
                }
            )
        );
        assert!(receiver.try_recv().is_err());
        decoder.feed(&vec![b'x'; MAX_LINE + 1]);
        decoder.feed(b"\n{\"event\":\"child_start\",\"sid\":\"root\",\"child_class\":\"hook\",\"child_id\":4,\"hook_name\":\"commit-msg\"}\n");
        assert_eq!(receiver.try_recv().unwrap().1.hook_name, "commit-msg");
    }
}
