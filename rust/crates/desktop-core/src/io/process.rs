//! Child processes for the renderer: a Node-style `child_process.spawn` that
//! the webview cannot do itself.
//!
//! The Copilot SDK starts its runtime over stdio and speaks JSON-RPC on the
//! pipes, so the bridge has to carry raw bytes both ways. Each child gets an
//! id. Output arrives as `desktop-process` events:
//!
//! * `{ id, event: "data", stream, data }` with base64 bytes,
//! * `{ id, event: "end", stream }` when a pipe closes,
//! * `{ id, event: "exit", code, signal }` once the child has exited and both
//!   output pipes are drained.
//!
//! Writes to stdin are queued to a thread per child, so the order the renderer
//! wrote them in is the order the child reads them.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{json, Value};

use crate::app::DesktopApp;
use crate::error::{AppError, Result};
use crate::storage::{base64_decode, base64_encode};

pub const EVENT: &str = "desktop-process";

/// Operations routed on the IPC worker itself rather than a helper thread, so
/// they run in the order the renderer issued them.
pub const PROCESS_OPERATIONS: &[&str] = &["process-write", "process-end", "process-kill"];

enum Input {
    Bytes(Vec<u8>),
    Close,
}

struct Entry {
    input: Sender<Input>,
    child: Arc<Mutex<Child>>,
}

fn registry() -> &'static Mutex<HashMap<u64, Entry>> {
    static REGISTRY: OnceLock<Mutex<HashMap<u64, Entry>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn string(o: &Value, key: &str) -> Result<String> {
    o.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::BadArgs(format!("missing `{key}`")))
}

fn id_of(o: &Value) -> Result<u64> {
    o.get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| AppError::BadArgs("missing `id`".into()))
}

fn entry_input(id: u64) -> Result<Sender<Input>> {
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .map(|entry| entry.input.clone())
        .ok_or_else(|| AppError::msg(format!("no running process with id {id}")))
}

/// Map a renderer path to a real one. Files under the virtual bundle root's
/// `copilot/` folder live in the installed Copilot SDK package.
pub fn resolve_command(app: &DesktopApp, command: &str) -> Result<PathBuf> {
    let root = crate::io::BUNDLE_ROOT;
    let Some(rest) = command.strip_prefix(&format!("{root}/")) else {
        return Ok(PathBuf::from(command));
    };
    let Some(copilot) = rest.strip_prefix("copilot/") else {
        return Err(not_found(command));
    };
    match &app.paths.copilot_dir {
        Some(dir) if !copilot.split('/').any(|part| part == "..") => Ok(dir.join(copilot)),
        _ => Err(not_found(command)),
    }
}

fn not_found(path: &str) -> AppError {
    AppError::Coded {
        code: "ENOENT".into(),
        message: format!("spawn {path} ENOENT"),
    }
}

pub fn spawn(app: &DesktopApp, o: &Value) -> Result<Value> {
    let command = resolve_command(app, &string(o, "command")?)?;
    let args: Vec<String> = o
        .get("args")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();

    let stdio: Vec<String> = o
        .get("stdio")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|v| v.as_str().unwrap_or("pipe").to_string()).collect())
        .unwrap_or_default();
    let piped = |index: usize| stdio.get(index).map(String::as_str).unwrap_or("pipe") != "ignore";

    let mut child_command = Command::new(&command);
    child_command.args(&args);
    if let Some(cwd) = o.get("cwd").and_then(Value::as_str).filter(|c| !c.is_empty()) {
        child_command.current_dir(cwd);
    }
    if let Some(env) = o.get("env").and_then(Value::as_object) {
        for (key, value) in env {
            match value.as_str() {
                Some(value) => child_command.env(key, value),
                None => child_command.env_remove(key),
            };
        }
    }
    child_command
        .stdin(if piped(0) { Stdio::piped() } else { Stdio::null() })
        .stdout(if piped(1) { Stdio::piped() } else { Stdio::null() })
        .stderr(if piped(2) { Stdio::piped() } else { Stdio::null() });

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        child_command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = child_command.spawn().map_err(|error| {
        let code = match error.kind() {
            std::io::ErrorKind::NotFound => "ENOENT",
            std::io::ErrorKind::PermissionDenied => "EACCES",
            _ => "EIO",
        };
        AppError::Coded {
            code: code.into(),
            message: format!("spawn {} {code}: {error}", command.display()),
        }
    })?;

    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let pid = child.id();
    let host = app.host();

    let (input_sender, input_receiver) = mpsc::channel::<Input>();
    let mut stdin = child.stdin.take();
    std::thread::Builder::new()
        .name(format!("desktop-process-in-{id}"))
        .spawn(move || {
            for message in input_receiver {
                match message {
                    Input::Bytes(bytes) => {
                        let Some(pipe) = stdin.as_mut() else { continue };
                        // A child that exited has no reader; its exit event follows.
                        if pipe.write_all(&bytes).and_then(|_| pipe.flush()).is_err() {
                            stdin = None;
                        }
                    }
                    Input::Close => {
                        stdin = None;
                    }
                }
            }
        })
        .map_err(AppError::Io)?;

    let mut readers: Vec<JoinHandle<()>> = Vec::new();
    for (stream, pipe) in [
        ("stdout", child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>)),
        ("stderr", child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>)),
    ] {
        let Some(mut pipe) = pipe else { continue };
        let host = host.clone();
        readers.push(
            std::thread::Builder::new()
                .name(format!("desktop-process-{stream}-{id}"))
                .spawn(move || {
                    let mut buffer = vec![0u8; 64 * 1024];
                    loop {
                        match pipe.read(&mut buffer) {
                            Ok(0) | Err(_) => break,
                            Ok(read) => host.emit(
                                EVENT,
                                json!([{
                                    "id": id,
                                    "event": "data",
                                    "stream": stream,
                                    "data": base64_encode(&buffer[..read]),
                                }]),
                            ),
                        }
                    }
                    host.emit(EVENT, json!([{ "id": id, "event": "end", "stream": stream }]));
                })
                .map_err(AppError::Io)?,
        );
    }

    let child = Arc::new(Mutex::new(child));
    registry().lock().unwrap_or_else(|e| e.into_inner()).insert(
        id,
        Entry { input: input_sender, child: child.clone() },
    );

    std::thread::Builder::new()
        .name(format!("desktop-process-wait-{id}"))
        .spawn(move || {
            let status = loop {
                let polled = child.lock().unwrap_or_else(|e| e.into_inner()).try_wait();
                match polled {
                    Ok(Some(status)) => break Some(status),
                    Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                    Err(_) => break None,
                }
            };
            // Exit is reported only after every output byte has been emitted.
            for reader in readers {
                let _ = reader.join();
            }
            registry().lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
            let (code, signal) = match status {
                Some(status) => (status.code(), signal_name(&status)),
                None => (None, None),
            };
            host.emit(
                EVENT,
                json!([{ "id": id, "event": "exit", "code": code, "signal": signal }]),
            );
        })
        .map_err(AppError::Io)?;

    Ok(json!({ "id": id, "pid": pid }))
}

#[cfg(unix)]
fn signal_name(status: &std::process::ExitStatus) -> Option<&'static str> {
    use std::os::unix::process::ExitStatusExt;
    Some(match status.signal()? {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        6 => "SIGABRT",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        15 => "SIGTERM",
        _ => "SIGUNKNOWN",
    })
}

#[cfg(not(unix))]
fn signal_name(_status: &std::process::ExitStatus) -> Option<&'static str> {
    None
}

pub fn write(o: &Value) -> Result<Value> {
    let input = entry_input(id_of(o)?)?;
    let bytes = base64_decode(&string(o, "data")?)?;
    // Queued, not written here: the IPC worker must not block on a full pipe.
    input
        .send(Input::Bytes(bytes))
        .map_err(|_| AppError::msg("the process has already exited"))?;
    Ok(Value::Null)
}

pub fn end_input(o: &Value) -> Result<Value> {
    let input = entry_input(id_of(o)?)?;
    let _ = input.send(Input::Close);
    Ok(Value::Null)
}

pub fn kill(o: &Value) -> Result<Value> {
    let id = id_of(o)?;
    let child = registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .map(|entry| entry.child.clone());
    let Some(child) = child else {
        return Ok(json!(false));
    };
    let mut child = child.lock().unwrap_or_else(|e| e.into_inner());
    let signal = o.get("signal").and_then(Value::as_str).unwrap_or("SIGTERM");

    // `Child::kill` always sends SIGKILL. Other signals go through `kill(1)`.
    #[cfg(unix)]
    if signal != "SIGKILL" {
        let flag = match signal {
            "SIGINT" => "-INT",
            "SIGHUP" => "-HUP",
            _ => "-TERM",
        };
        let sent = Command::new("kill")
            .args([flag, &child.id().to_string()])
            .status()
            .is_ok_and(|status| status.success());
        return Ok(json!(sent));
    }
    let _ = signal;
    Ok(json!(child.kill().is_ok()))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::ipc::protocol::{Host, WindowAction};
    use std::time::Instant;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<Value>>);

    impl Host for Recorder {
        fn emit(&self, channel: &str, args: Value) {
            if channel == EVENT {
                self.0.lock().unwrap().push(args[0].clone());
            }
        }
        fn window(&self, _action: WindowAction) -> Result<Value> {
            Ok(Value::Null)
        }
    }

    #[test]
    fn stdin_round_trips_through_a_child_and_exit_is_reported_last() {
        let recorder = Arc::new(Recorder::default());
        let app = crate::app::DesktopApp::new(recorder.clone()).unwrap();

        let spawned = spawn(&app, &json!({ "command": "cat", "args": [], "stdio": ["pipe", "pipe", "pipe"] })).unwrap();
        let id = spawned["id"].as_u64().unwrap();

        write(&json!({ "id": id, "data": base64_encode(b"hello, ") })).unwrap();
        write(&json!({ "id": id, "data": base64_encode(&[0u8, 255, 10]) })).unwrap();
        end_input(&json!({ "id": id })).unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let events = recorder.0.lock().unwrap().clone();
            if events.iter().any(|e| e["event"] == "exit") {
                let mut output = Vec::new();
                for event in &events {
                    if event["event"] == "data" && event["stream"] == "stdout" {
                        output.extend(base64_decode(event["data"].as_str().unwrap()).unwrap());
                    }
                }
                assert_eq!(output, [b"hello, ".as_slice(), &[0u8, 255, 10]].concat());
                assert_eq!(events.last().unwrap()["event"], "exit", "exit comes after the output");
                assert_eq!(events.last().unwrap()["code"], 0);
                break;
            }
            assert!(Instant::now() < deadline, "the child did not exit");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn missing_programs_fail_with_node_error_codes() {
        let app = crate::app::DesktopApp::new(Arc::new(Recorder::default())).unwrap();
        let error = spawn(&app, &json!({ "command": "/definitely/not/a/program" })).unwrap_err();
        assert_eq!(error.to_rpc_error().code.as_deref(), Some("ENOENT"));
    }

    #[test]
    fn bundle_paths_map_only_into_the_copilot_package() {
        let app = crate::app::DesktopApp::new(Arc::new(Recorder::default())).unwrap();
        assert_eq!(resolve_command(&app, "/usr/bin/git").unwrap(), PathBuf::from("/usr/bin/git"));
        assert!(resolve_command(&app, "/__desktop_app__/index.html").is_err());
        assert!(resolve_command(&app, "/__desktop_app__/copilot/../../etc/passwd").is_err());
    }
}
