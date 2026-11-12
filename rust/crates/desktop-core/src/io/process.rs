//! Child processes for the renderer: a Node-style `child_process.spawn` that
//! the webview cannot do itself.
//!
//! The Copilot SDK starts its runtime over stdio and speaks JSON-RPC on the
//! pipes, so the bridge has to carry raw bytes both ways. Each child gets an
//! id. Output arrives as `desktop-process` events:
//!
//! * `{ id, event: "data", stream, seq, data }` with base64 bytes,
//! * `{ id, event: "end", stream }` when a pipe closes,
//! * `{ id, event: "drain" }` when stdin has room for another block,
//! * `{ id, event: "exit", code, signal }` once the child has exited and both
//!   output pipes are drained.
//!
//! Flow control, in both directions:
//!
//! * **Input.** One block (at most [`STREAM_BLOCK`]) is held for the writer
//!   thread at a time. A `write` while that block is still pending is refused
//!   with `EBUSY`/`QUEUE_FULL`, and the next `drain` event says when to retry.
//!   The JavaScript side keeps its own bounded queue, so no byte is dropped.
//! * **Output.** Each data block takes credit from its stream's window and
//!   from the global output budget. The reader stops reading the pipe when the
//!   credit is used up, so the child blocks on its own write. Credit comes back
//!   only when the renderer acknowledges a sequence number cumulatively.
//!
//! Every reservation is a [`Permit`], so credit returns exactly once when it is
//! acknowledged or when its owner is dropped.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{Value, json};

use crate::app::DesktopApp;
use crate::error::{AppError, Result};
use crate::io::limits::{ByteBudget, Permit, STREAM_BLOCK};
use crate::ipc::protocol::Host;
use crate::storage::{base64_decode, base64_encode};

pub const EVENT: &str = "desktop-process";

/// Longest a reaper sleeps between `try_wait` calls for a running child.
const MAX_REAP_POLL: Duration = Duration::from_millis(50);

fn encoded_len(raw: usize) -> usize {
    raw.div_ceil(3) * 4
}

fn busy(message: &str) -> AppError {
    AppError::Coded {
        code: "EBUSY".into(),
        message: format!("EBUSY: QUEUE_FULL, {message}"),
    }
}

// ---------------------------------------------------------------------------
// input
// ---------------------------------------------------------------------------

#[derive(Default)]
struct InputState {
    /// The one block waiting for the writer, with the global budget it holds.
    block: Option<(Vec<u8>, Permit)>,
    /// The renderer asked for end-of-input. The pending block still goes out first.
    closed: bool,
    /// The pipe is gone or the process was killed; nothing more is accepted.
    broken: bool,
    /// A `push` was refused, so the writer must report `drain` when it frees room.
    waiting_drain: bool,
}

struct InputGate {
    state: Mutex<InputState>,
    ready: Condvar,
    global: Arc<ByteBudget>,
}

#[derive(Debug, PartialEq, Eq)]
enum PushError {
    /// The previous block has not been written yet.
    Full,
    /// Input was already ended.
    Ended,
    /// The process cannot take input any more.
    Refused,
    /// The block is larger than the block size; the caller must split it.
    TooLarge,
}

impl InputGate {
    fn new(global: Arc<ByteBudget>, writable: bool) -> Arc<Self> {
        let state = InputState {
            broken: !writable,
            ..InputState::default()
        };
        Arc::new(Self {
            state: Mutex::new(state),
            ready: Condvar::new(),
            global,
        })
    }

    fn push(&self, data: Vec<u8>) -> std::result::Result<(), PushError> {
        if data.len() > STREAM_BLOCK {
            return Err(PushError::TooLarge);
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.broken {
            return Err(PushError::Refused);
        }
        if state.closed {
            return Err(PushError::Ended);
        }
        if state.block.is_some() {
            state.waiting_drain = true;
            return Err(PushError::Full);
        }
        let Some(permit) = self.global.try_acquire(data.len()) else {
            state.waiting_drain = true;
            return Err(PushError::Full);
        };
        state.block = Some((data, permit));
        drop(state);
        self.ready.notify_one();
        Ok(())
    }

    fn end(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
        self.ready.notify_all();
    }

    /// Stop accepting input and discard the pending block. Returns whether a
    /// producer was waiting for `drain`, so the caller can wake it.
    fn refuse(&self) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.broken = true;
        state.block = None;
        let wake = std::mem::take(&mut state.waiting_drain);
        drop(state);
        self.ready.notify_all();
        wake
    }
}

/// Owns the child's stdin. Exits when the input ends, the pipe fails, or the
/// input is refused. Dropping `stdin` on the way out delivers EOF.
fn run_writer(mut stdin: Option<ChildStdin>, gate: Arc<InputGate>, host: Arc<dyn Host>, id: u64) {
    loop {
        let block = {
            let mut state = gate.state.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if state.broken {
                    break None;
                }
                if let Some(block) = state.block.take() {
                    break Some(block);
                }
                if state.closed {
                    break None;
                }
                state = gate.ready.wait(state).unwrap_or_else(|e| e.into_inner());
            }
        };
        let Some((bytes, permit)) = block else { break };

        // A child that exited has no reader; its exit event follows.
        let written = match stdin.as_mut() {
            Some(pipe) => pipe.write_all(&bytes).and_then(|_| pipe.flush()).is_ok(),
            None => false,
        };
        drop(permit);

        let (wake, done) = {
            let mut state = gate.state.lock().unwrap_or_else(|e| e.into_inner());
            if !written {
                state.broken = true;
            }
            (std::mem::take(&mut state.waiting_drain), state.broken)
        };
        if wake {
            host.emit(EVENT, json!([{ "id": id, "event": "drain" }]));
        }
        if done {
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// output
// ---------------------------------------------------------------------------

#[derive(Default)]
struct OutputState {
    /// Sequence number of the last block handed to the renderer.
    sent: u64,
    /// Highest sequence number the renderer has acknowledged.
    acked: u64,
    /// Blocks not yet acknowledged, with the credit each one holds.
    in_flight: VecDeque<(u64, Permit, Permit)>,
}

struct OutputGate {
    window: Arc<ByteBudget>,
    global: Arc<ByteBudget>,
    state: Mutex<OutputState>,
}

impl OutputGate {
    fn new(window: usize, global: Arc<ByteBudget>) -> Arc<Self> {
        Arc::new(Self {
            window: ByteBudget::new(window),
            global,
            state: Mutex::new(OutputState::default()),
        })
    }

    /// Record a block that was just sent and keep its credit until it is acked.
    fn record(&self, window: Permit, global: Permit) -> u64 {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.sent += 1;
        let seq = state.sent;
        state.in_flight.push_back((seq, window, global));
        seq
    }

    /// Apply a cumulative acknowledgement. Duplicate, stale or unknown
    /// sequence numbers release nothing.
    fn ack(&self, seq: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if seq <= state.acked || seq > state.sent {
            return;
        }
        while state
            .in_flight
            .front()
            .is_some_and(|(front, _, _)| *front <= seq)
        {
            // Dropping the permits returns their credit.
            state.in_flight.pop_front();
        }
        state.acked = seq;
    }
}

/// Read one pipe to its end, sending each block as a credited data event.
/// Stops early only when the window is closed (renderer shutdown).
fn run_reader(
    mut pipe: Box<dyn Read + Send>,
    stream: &'static str,
    gate: Arc<OutputGate>,
    host: Arc<dyn Host>,
    id: u64,
) {
    let mut buffer = vec![0u8; STREAM_BLOCK];
    loop {
        let read = match pipe.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        let cost = encoded_len(read);
        let Some(window) = gate.window.acquire(cost) else {
            break;
        };
        let Some(global) = gate.global.acquire(cost) else {
            break;
        };
        let seq = gate.record(window, global);
        host.emit(
            EVENT,
            json!([{
                "id": id,
                "event": "data",
                "stream": stream,
                "seq": seq,
                "data": base64_encode(&buffer[..read]),
            }]),
        );
    }
    host.emit(
        EVENT,
        json!([{ "id": id, "event": "end", "stream": stream }]),
    );
}

// ---------------------------------------------------------------------------
// registry
// ---------------------------------------------------------------------------

struct Entry {
    input: Arc<InputGate>,
    child: Arc<Mutex<Child>>,
    outputs: [Arc<OutputGate>; 2],
    /// The process-slot budget of the application that spawned the child.
    owner: Arc<ByteBudget>,
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

/// Clone out what a request needs, so the registry lock is never held while
/// touching pipes or the child.
type ProcessHandles = (Arc<InputGate>, Arc<Mutex<Child>>, [Arc<OutputGate>; 2]);

fn lookup(id: u64) -> Option<ProcessHandles> {
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .map(|entry| {
            (
                entry.input.clone(),
                entry.child.clone(),
                entry.outputs.clone(),
            )
        })
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

/// Start a child. Only the startup occupies the caller; the child's lifetime
/// holds one process slot until its exit has been reported.
pub fn spawn(app: &DesktopApp, o: &Value) -> Result<Value> {
    let slot = app
        .resources
        .process_slots
        .try_acquire(1)
        .ok_or_else(|| busy("too many persistent processes are running"))?;

    let command = resolve_command(app, &string(o, "command")?)?;
    let args: Vec<String> = o
        .get("args")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let stdio: Vec<String> = o
        .get("stdio")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|v| v.as_str().unwrap_or("pipe").to_string())
                .collect()
        })
        .unwrap_or_default();
    let piped = |index: usize| stdio.get(index).map(String::as_str).unwrap_or("pipe") != "ignore";

    let mut child_command = Command::new(&command);
    child_command.args(&args);
    if let Some(cwd) = o
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty())
    {
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
        .stdin(if piped(0) {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(if piped(1) {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(if piped(2) {
            Stdio::piped()
        } else {
            Stdio::null()
        });

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
    let limits = app.resources.limits;

    let input = InputGate::new(app.resources.input.clone(), child.stdin.is_some());
    let outputs = [
        OutputGate::new(limits.stream_window(), app.resources.output.clone()),
        OutputGate::new(limits.stream_window(), app.resources.output.clone()),
    ];

    // Everything below may fail after the child exists. On failure the child
    // is killed and reaped here, so nothing is left running without a registry
    // entry.
    let abandon = |child: &mut Child, input: &InputGate, error: std::io::Error| {
        input.end();
        let _ = child.kill();
        let _ = child.wait();
        AppError::Io(error)
    };

    let stdin = child.stdin.take();
    let writer = {
        let input = input.clone();
        let host = host.clone();
        std::thread::Builder::new()
            .name(format!("desktop-process-in-{id}"))
            .spawn(move || run_writer(stdin, input, host, id))
    };
    if let Err(error) = writer {
        return Err(abandon(&mut child, &input, error));
    }

    let mut readers: Vec<JoinHandle<()>> = Vec::new();
    let pipes: [(&'static str, Option<Box<dyn Read + Send>>); 2] = [
        (
            "stdout",
            child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
        ),
        (
            "stderr",
            child
                .stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
        ),
    ];
    for (index, (stream, pipe)) in pipes.into_iter().enumerate() {
        let Some(pipe) = pipe else { continue };
        let gate = outputs[index].clone();
        let host = host.clone();
        match std::thread::Builder::new()
            .name(format!("desktop-process-{stream}-{id}"))
            .spawn(move || run_reader(pipe, stream, gate, host, id))
        {
            Ok(handle) => readers.push(handle),
            Err(error) => {
                for reader in readers {
                    let _ = reader.join();
                }
                return Err(abandon(&mut child, &input, error));
            }
        }
    }

    let child = Arc::new(Mutex::new(child));
    registry().lock().unwrap_or_else(|e| e.into_inner()).insert(
        id,
        Entry {
            input: input.clone(),
            child: child.clone(),
            outputs,
            owner: app.resources.process_slots.clone(),
        },
    );

    let reaper = {
        let host = host.clone();
        let input = input.clone();
        std::thread::Builder::new()
            .name(format!("desktop-process-wait-{id}"))
            .spawn(move || {
                let status = wait_for_exit(&child);
                // The exit is reported only after the last output block is
                // acknowledged, because the readers only finish after that.
                for reader in readers {
                    let _ = reader.join();
                }
                input.refuse();
                registry()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                let (code, signal) = match status {
                    Some(status) => (status.code(), signal_name(&status)),
                    None => (None, None),
                };
                host.emit(
                    EVENT,
                    json!([{ "id": id, "event": "exit", "code": code, "signal": signal }]),
                );
                // Returning the slot last keeps the count honest until the
                // exit is out.
                drop(slot);
            })
    };
    if let Err(error) = reaper {
        // The reaper never started, so its closure (and the slot) was dropped
        // here; the registry entry is still there and must go.
        let entry = registry()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        if let Some(entry) = entry {
            entry.input.refuse();
            let _ = entry.child.lock().unwrap_or_else(|e| e.into_inner()).kill();
        }
        return Err(AppError::Io(error));
    }

    let limits_json = json!({
        "block": STREAM_BLOCK,
        "inputBudget": limits.js_input_budget(),
        "outputWindow": limits.stream_window(),
    });
    Ok(json!({ "id": id, "pid": pid, "limits": limits_json }))
}

/// Poll a child with a bounded, growing delay. Output is not delayed by this:
/// readers run on their own threads and report data as it arrives.
fn wait_for_exit(child: &Mutex<Child>) -> Option<ExitStatus> {
    let mut delay = Duration::from_millis(1);
    loop {
        let polled = child.lock().unwrap_or_else(|e| e.into_inner()).try_wait();
        match polled {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {
                std::thread::sleep(delay);
                delay = (delay * 2).min(MAX_REAP_POLL);
            }
            Err(_) => return None,
        }
    }
}

#[cfg(unix)]
fn signal_name(status: &ExitStatus) -> Option<&'static str> {
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
fn signal_name(_status: &ExitStatus) -> Option<&'static str> {
    None
}

/// Queue one block of stdin. Never waits: a block that cannot be taken yet is
/// refused with `EBUSY`, and the caller retries after the `drain` event.
pub fn write(o: &Value) -> Result<Value> {
    let id = id_of(o)?;
    let bytes = base64_decode(&string(o, "data")?)?;
    let Some((input, _, _)) = lookup(id) else {
        return Err(AppError::msg("the process has already exited"));
    };
    match input.push(bytes) {
        Ok(()) => Ok(Value::Null),
        Err(PushError::Full) => Err(busy("the process has not consumed its pending input")),
        Err(PushError::Ended) => Err(AppError::Coded {
            code: "EPIPE".into(),
            message: "EPIPE: write after end".into(),
        }),
        Err(PushError::Refused) => Err(AppError::msg("the process has already exited")),
        Err(PushError::TooLarge) => Err(AppError::BadArgs(format!(
            "stdin block larger than {STREAM_BLOCK} bytes; split it"
        ))),
    }
}

/// End stdin once everything already accepted has been written. The request
/// only records the state, so it never waits behind a full pipe.
pub fn end_input(o: &Value) -> Result<Value> {
    let id = id_of(o)?;
    if let Some((input, _, _)) = lookup(id) {
        input.end();
    }
    Ok(Value::Null)
}

/// Acknowledge output up to `seq` for one stream. Returns credit only for
/// blocks that were actually sent; anything else is ignored.
pub fn ack(o: &Value) -> Result<Value> {
    let id = id_of(o)?;
    let seq = o
        .get("seq")
        .and_then(Value::as_u64)
        .ok_or_else(|| AppError::BadArgs("missing `seq`".into()))?;
    let index = match o.get("stream").and_then(Value::as_str) {
        Some("stdout") => 0,
        Some("stderr") => 1,
        _ => {
            return Err(AppError::BadArgs(
                "`stream` must be stdout or stderr".into(),
            ));
        }
    };
    if let Some((_, _, outputs)) = lookup(id) {
        outputs[index].ack(seq);
    }
    Ok(Value::Null)
}

/// Stop a child. Pending stdin is discarded and any producer waiting for
/// `drain` is woken. Returns whether the signal was sent.
pub fn kill(app: &DesktopApp, o: &Value) -> Result<Value> {
    let id = id_of(o)?;
    let Some((input, child, _)) = lookup(id) else {
        return Ok(json!(false));
    };
    if input.refuse() {
        app.host()
            .emit(EVENT, json!([{ "id": id, "event": "drain" }]));
    }
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

/// Renderer shutdown: stop every child and unblock every stream waiting for
/// credit, so no reader or writer outlives the window that owned it.
pub fn shutdown(app: &DesktopApp) {
    // Only this application's children: the registry is shared by every
    // application in the process (tests included).
    let entries: Vec<(u64, ProcessHandles)> = registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|(_, entry)| Arc::ptr_eq(&entry.owner, &app.resources.process_slots))
        .map(|(id, entry)| {
            (
                *id,
                (
                    entry.input.clone(),
                    entry.child.clone(),
                    entry.outputs.clone(),
                ),
            )
        })
        .collect();
    for (id, (input, child, outputs)) in entries {
        if input.refuse() {
            app.host()
                .emit(EVENT, json!([{ "id": id, "event": "drain" }]));
        }
        for output in &outputs {
            output.window.close();
        }
        let _ = child.lock().unwrap_or_else(|e| e.into_inner()).kill();
    }
    app.resources.process_slots.close();
    app.resources.input.close();
    app.resources.output.close();
    app.resources.results.close();
}

/// Live counters for development tooling. Carries no stream content.
pub fn diagnostics(app: &DesktopApp) -> Value {
    let processes = registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .filter(|entry| Arc::ptr_eq(&entry.owner, &app.resources.process_slots))
        .count();
    json!({
        "processes": processes,
        "processSlots": {
            "used": app.resources.process_slots.used(),
            "capacity": app.resources.process_slots.capacity(),
        },
        "inputBytes": app.resources.input.used(),
        "outputBytes": app.resources.output.used(),
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::error::Result as AppResult;
    use crate::ipc::protocol::WindowAction;
    use std::time::Instant;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<Value>>);

    impl Host for Recorder {
        fn emit(&self, channel: &str, args: Value) {
            if channel == EVENT {
                self.0.lock().unwrap().push(args[0].clone());
            }
        }
        fn window(&self, _action: WindowAction) -> AppResult<Value> {
            Ok(Value::Null)
        }
    }

    fn app(recorder: &Arc<Recorder>) -> Arc<DesktopApp> {
        DesktopApp::new(recorder.clone()).unwrap()
    }

    /// Send `bytes` in blocks, retrying refused blocks the way the renderer
    /// does after a `drain` event (here: a short poll).
    fn write_blocks(id: u64, bytes: &[u8]) {
        for block in bytes.chunks(STREAM_BLOCK) {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                match write(&json!({ "id": id, "data": base64_encode(block) })) {
                    Ok(_) => break,
                    Err(error) if error.to_rpc_error().code.as_deref() == Some("EBUSY") => {
                        assert!(Instant::now() < deadline, "stdin never drained");
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("unexpected write error: {error}"),
                }
            }
        }
    }

    /// Acknowledge every data block seen so far, as the renderer does.
    fn ack_seen(recorder: &Recorder, id: u64) {
        let mut newest = std::collections::HashMap::<String, u64>::new();
        for event in recorder.0.lock().unwrap().iter() {
            if event["id"] == id && event["event"] == "data" {
                let stream = event["stream"].as_str().unwrap().to_string();
                let seq = event["seq"].as_u64().unwrap();
                let slot = newest.entry(stream).or_insert(0);
                *slot = (*slot).max(seq);
            }
        }
        for (stream, seq) in newest {
            ack(&json!({ "id": id, "stream": stream, "seq": seq })).unwrap();
        }
    }

    fn output_of(recorder: &Recorder, id: u64, stream: &str) -> Vec<u8> {
        let mut output = Vec::new();
        for event in recorder.0.lock().unwrap().iter() {
            if event["id"] == id && event["event"] == "data" && event["stream"] == stream {
                output.extend(base64_decode(event["data"].as_str().unwrap()).unwrap());
            }
        }
        output
    }

    fn exit_seen(recorder: &Recorder, id: u64) -> bool {
        recorder
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|e| e["id"] == id && e["event"] == "exit")
    }

    fn wait_for(recorder: &Recorder, id: u64, deadline: Instant, what: &str) {
        while !exit_seen(recorder, id) {
            assert!(Instant::now() < deadline, "{what}: the child did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn stdin_round_trips_through_a_child_and_exit_is_reported_last() {
        let recorder = Arc::new(Recorder::default());
        let app = app(&recorder);

        let spawned = spawn(
            &app,
            &json!({ "command": "cat", "args": [], "stdio": ["pipe", "pipe", "pipe"] }),
        )
        .unwrap();
        let id = spawned["id"].as_u64().unwrap();

        write_blocks(id, b"hello, ");
        write_blocks(id, &[0u8, 255, 10]);
        end_input(&json!({ "id": id })).unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        while !exit_seen(&recorder, id) {
            ack_seen(&recorder, id);
            assert!(Instant::now() < deadline, "the child did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            output_of(&recorder, id, "stdout"),
            [b"hello, ".as_slice(), &[0u8, 255, 10]].concat()
        );
        let events = recorder.0.lock().unwrap().clone();
        assert_eq!(
            events.last().unwrap()["event"],
            "exit",
            "exit comes after the output"
        );
        assert_eq!(events.last().unwrap()["code"], 0);
    }

    #[test]
    fn a_full_stdin_refuses_with_quota_error_then_drains() {
        // `sleep` never reads stdin, so the pipe fills and the writer stalls.
        let recorder = Arc::new(Recorder::default());
        let app = app(&recorder);
        let spawned = spawn(
            &app,
            &json!({ "command": "sleep", "args": ["30"], "stdio": ["pipe", "pipe", "pipe"] }),
        )
        .unwrap();
        let id = spawned["id"].as_u64().unwrap();

        let block = base64_encode(&vec![7u8; STREAM_BLOCK]);
        let mut refused = None;
        for _ in 0..16 {
            match write(&json!({ "id": id, "data": block.clone() })) {
                Ok(_) => {}
                Err(error) => {
                    refused = Some(error.to_rpc_error());
                    break;
                }
            }
        }
        let refused = refused.expect("a stalled child must eventually refuse input");
        assert_eq!(refused.code.as_deref(), Some("EBUSY"));
        assert!(refused.message.contains("QUEUE_FULL"));

        kill(&app, &json!({ "id": id, "signal": "SIGKILL" })).unwrap();
        wait_for(
            &recorder,
            id,
            Instant::now() + Duration::from_secs(10),
            "after kill",
        );
    }

    #[test]
    fn oversized_blocks_and_unknown_streams_are_rejected() {
        let recorder = Arc::new(Recorder::default());
        let app = app(&recorder);
        let spawned = spawn(
            &app,
            &json!({ "command": "cat", "args": [], "stdio": ["pipe", "pipe", "pipe"] }),
        )
        .unwrap();
        let id = spawned["id"].as_u64().unwrap();

        let too_big = base64_encode(&vec![0u8; STREAM_BLOCK + 1]);
        assert!(matches!(
            write(&json!({ "id": id, "data": too_big })),
            Err(AppError::BadArgs(_))
        ));
        assert!(ack(&json!({ "id": id, "stream": "stdin", "seq": 1 })).is_err());

        end_input(&json!({ "id": id })).unwrap();
        wait_for(
            &recorder,
            id,
            Instant::now() + Duration::from_secs(10),
            "cat after EOF",
        );
    }

    #[test]
    fn echoing_more_than_one_window_completes_when_acknowledged() {
        // 600 KiB through `cat` is more than a 256 KiB stdout window, so the
        // child can only finish if acknowledgements return credit.
        let recorder = Arc::new(Recorder::default());
        let app = app(&recorder);
        let spawned = spawn(
            &app,
            &json!({ "command": "cat", "args": [], "stdio": ["pipe", "pipe", "pipe"] }),
        )
        .unwrap();
        let id = spawned["id"].as_u64().unwrap();

        let payload: Vec<u8> = (0..600 * 1024).map(|i| (i % 251) as u8).collect();
        let writer_payload = payload.clone();
        let writer = std::thread::spawn(move || {
            write_blocks(id, &writer_payload);
            end_input(&json!({ "id": id })).unwrap();
        });

        let deadline = Instant::now() + Duration::from_secs(30);
        while !exit_seen(&recorder, id) {
            ack_seen(&recorder, id);
            assert!(Instant::now() < deadline, "flow control stalled");
            std::thread::sleep(Duration::from_millis(5));
        }
        writer.join().unwrap();
        assert_eq!(output_of(&recorder, id, "stdout"), payload);
    }

    #[test]
    fn stale_or_future_acks_release_nothing() {
        let recorder = Arc::new(Recorder::default());
        let app = app(&recorder);
        let spawned = spawn(&app, &json!({ "command": "sh", "args": ["-c", "printf abc"], "stdio": ["pipe", "pipe", "pipe"] })).unwrap();
        let id = spawned["id"].as_u64().unwrap();
        wait_for(
            &recorder,
            id,
            Instant::now() + Duration::from_secs(10),
            "printf",
        );

        // An acknowledgement beyond what was sent must not free anything, and
        // acknowledging twice must not free twice.
        ack(&json!({ "id": id, "stream": "stdout", "seq": 999 })).unwrap();
        let first = recorder
            .0
            .lock()
            .unwrap()
            .iter()
            .find(|e| e["event"] == "data")
            .cloned()
            .unwrap();
        let seq = first["seq"].as_u64().unwrap();
        ack(&json!({ "id": id, "stream": "stdout", "seq": seq })).unwrap();
        ack(&json!({ "id": id, "stream": "stdout", "seq": seq })).unwrap();
        assert_eq!(output_of(&recorder, id, "stdout"), b"abc");
    }

    #[test]
    fn missing_programs_fail_with_node_error_codes() {
        let recorder = Arc::new(Recorder::default());
        let app = app(&recorder);
        let error = spawn(&app, &json!({ "command": "/definitely/not/a/program" })).unwrap_err();
        assert_eq!(error.to_rpc_error().code.as_deref(), Some("ENOENT"));
        assert_eq!(
            app.resources.process_slots.used(),
            0,
            "a failed spawn gives its slot back"
        );
    }

    #[test]
    fn process_slots_are_bounded_and_returned_on_exit() {
        let recorder = Arc::new(Recorder::default());
        let app = app(&recorder);
        let capacity = app.resources.process_slots.capacity();
        let mut ids = Vec::new();
        for _ in 0..capacity {
            let spawned = spawn(&app, &json!({ "command": "sleep", "args": ["30"] })).unwrap();
            ids.push(spawned["id"].as_u64().unwrap());
        }
        let refused = spawn(&app, &json!({ "command": "sleep", "args": ["30"] })).unwrap_err();
        assert_eq!(refused.to_rpc_error().code.as_deref(), Some("EBUSY"));

        for id in &ids {
            kill(&app, &json!({ "id": id, "signal": "SIGKILL" })).unwrap();
            wait_for(
                &recorder,
                *id,
                Instant::now() + Duration::from_secs(10),
                "sleep",
            );
        }
        assert_eq!(app.resources.process_slots.used(), 0);
        assert!(spawn(&app, &json!({ "command": "true" })).is_ok());
    }

    #[test]
    fn shutdown_stops_every_child_and_frees_the_budgets() {
        let recorder = Arc::new(Recorder::default());
        let app = app(&recorder);
        let spawned = spawn(&app, &json!({ "command": "sleep", "args": ["30"] })).unwrap();
        let id = spawned["id"].as_u64().unwrap();
        shutdown(&app);
        wait_for(
            &recorder,
            id,
            Instant::now() + Duration::from_secs(10),
            "shutdown",
        );
    }

    /// The Copilot runtime, spoken to exactly as the SDK does over stdio:
    /// framed JSON-RPC on the child's pipes, through the bridge.
    #[test]
    fn copilot_runtime_answers_a_framed_ping_through_the_bridge() {
        let recorder = Arc::new(Recorder::default());
        let app = app(&recorder);
        let runtime = resolve_command(
            &app,
            "/__desktop_app__/copilot/prebuilds/linux-x64/copilot-runtime",
        )
        .expect("the Copilot SDK platform package must be installed for this test");
        assert!(
            runtime.is_file(),
            "missing runtime at {}",
            runtime.display()
        );

        let spawned = spawn(
            &app,
            &json!({
                "command": "/__desktop_app__/copilot/prebuilds/linux-x64/copilot-runtime",
                "args": ["--headless", "--no-auto-update", "--stdio", "--no-auto-login"],
                "stdio": ["pipe", "pipe", "pipe"],
            }),
        )
        .unwrap();
        let id = spawned["id"].as_u64().unwrap();

        let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{"message":"probe"}}"#;
        let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        write_blocks(id, frame.as_bytes());

        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            ack_seen(&recorder, id);
            let output = output_of(&recorder, id, "stdout");
            let text = String::from_utf8_lossy(&output).into_owned();
            if text.contains("\"result\"") {
                assert!(text.contains("pong: probe"), "unexpected reply: {text}");
                break;
            }
            let exited = exit_seen(&recorder, id);
            if exited || Instant::now() > deadline {
                let stderr =
                    String::from_utf8_lossy(&output_of(&recorder, id, "stderr")).into_owned();
                panic!("no ping reply; exited={exited}; stdout={text:?}; stderr={stderr:?}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        kill(&app, &json!({ "id": id, "signal": "SIGKILL" })).unwrap();
    }
}
