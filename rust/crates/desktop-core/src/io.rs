//! Host services for the legacy renderer.
//!
//! Electron's renderer ran with Node integration and called `dugite`, `fs`,
//! `keytar` and an unrestricted `fetch` directly. A webview has none of
//! those, so the web build's shims (`app/src/web/*`) forward each call to a
//! `desktop.io.*` command implemented here.
//!
//! These commands are stateless: they never touch [`AppState`], so the shell
//! runs them on their own threads. A long clone or HTTP request must not
//! block the IPC worker or the state lock.
//!
//! [`AppState`]: crate::models::app_state::AppState

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::app::DesktopApp;
use crate::error::{AppError, Result};
use crate::io::limits::{ByteBudget, Permit, Resources, STREAM_BLOCK};
use sha2::Digest;
use crate::storage::{base64_decode, base64_encode};

pub const PREFIX: &str = "desktop.io.";

pub mod executor;
pub mod limits;
mod process;

pub use process::EVENT as PROCESS_EVENT;

/// The renderer's `__dirname` in the web build (`BundleRoot` in
/// `app/src/web/bundle.ts`). Not a real directory: the shims map paths under
/// it onto resources served from the page origin.
pub const BUNDLE_ROOT: &str = "/__desktop_app__";

/// Which executor serves a host operation. The lanes are separate so that a
/// slow operation in one cannot starve the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    /// Process stdin, output acknowledgements and kill. Each request is tiny
    /// and never waits for the child, so this lane always makes progress.
    Control,
    /// Operations that may run for a long time: git, HTTP, process spawn,
    /// hashing and recursive file work.
    Long,
    /// Everything else: metadata, ranges, small file operations, keychain.
    Short,
}

/// Whether `channel` is served by this module.
pub fn handles(channel: &str) -> bool {
    channel.starts_with(PREFIX)
}

/// The lane for a `desktop.io.*` channel. Unknown channels are `Short`, so
/// they fail quickly with `UnknownChannel`.
pub fn lane(channel: &str) -> Lane {
    match channel.strip_prefix(PREFIX).unwrap_or("") {
        "process-write" | "process-end" | "process-kill" | "process-ack" | "cancel" => Lane::Control,
        "git-exec" | "http-fetch" | "process-spawn" | "fs-copy" | "fs-copy-file" | "fs-hash" => Lane::Long,
        "fs-rm" => Lane::Long,
        _ => Lane::Short,
    }
}

/// Synchronous (`invokeSync`) calls may use only short work. A recursive
/// removal is long work here: it keeps the renderer blocked, so the caller
/// must use the asynchronous API.
pub fn allowed_synchronously(channel: &str, args: &[Value]) -> bool {
    if !handles(channel) {
        // Renderer state commands are asynchronous only.
        return false;
    }
    let recursive = args.first().and_then(|o| o.get("recursive")).and_then(Value::as_bool).unwrap_or(false);
    match lane(channel) {
        Lane::Short => true,
        Lane::Control => false,
        Lane::Long => channel == "desktop.io.fs-rm" && !recursive,
    }
}

pub fn invoke(app: &DesktopApp, channel: &str, args: &[Value]) -> Result<Value> {
    let empty = json!({});
    let o = args.first().unwrap_or(&empty);
    match channel.strip_prefix(PREFIX).unwrap_or("") {
        "git-exec" => git_exec(&app.git.binary, &app.resources, o),
        "http-fetch" => http_fetch(&app.resources, o),
        "fs-read-range" => read_range(o),
        "fs-hash" => file_hash(o),
        "cancel" => cancel(o),
        "keychain-get" => Ok(app
            .secure
            .get_item(&keychain_key(o)?, &string(o, "account")?)?
            .map(Value::String)
            .unwrap_or(Value::Null)),
        "keychain-set" => {
            app.secure
                .set_item(&keychain_key(o)?, &string(o, "account")?, &string(o, "password")?)?;
            Ok(Value::Null)
        }
        "keychain-delete" => {
            app.secure.delete_item(&keychain_key(o)?, &string(o, "account")?)?;
            Ok(json!(true))
        }
        "process-spawn" => process::spawn(app, o),
        "process-write" => process::write(o),
        "process-end" => process::end_input(o),
        "process-ack" => process::ack(o),
        "process-kill" => process::kill(app, o),
        other => match other.strip_prefix("fs-") {
            Some(operation) => fs_invoke(operation, o),
            None => Err(AppError::UnknownChannel(channel.to_string())),
        },
    }
}

/// Renderer shutdown: stop children and release every stream waiting for
/// credit. See [`process::shutdown`].
pub fn shutdown(app: &DesktopApp) {
    process::shutdown(app);
}

/// Live counters for development tooling. Holds no stream content.
pub fn diagnostics(app: &DesktopApp) -> Value {
    json!({ "processes": process::diagnostics(app) })
}

fn string(o: &Value, key: &str) -> Result<String> {
    o.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::BadArgs(format!("missing `{key}`")))
}

fn flag(o: &Value, key: &str) -> bool {
    o.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// keytar addresses a secret by (service, account). The store hashes one key.
fn keychain_key(o: &Value) -> Result<String> {
    Ok(format!("{}\n{}", string(o, "service")?, string(o, "account")?))
}

// ---------------------------------------------------------------------------
// git
// ---------------------------------------------------------------------------

/// Why a running command was stopped before it finished. The first reason
/// recorded is the one reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// More output than the caller's `maxBuffer`.
    Requested,
    /// More output than the per-command retention ceiling.
    Ceiling,
    /// The budget for retained results is exhausted.
    Budget,
    /// The renderer cancelled the request.
    Cancelled,
}

type StopCell = Arc<Mutex<Option<Stop>>>;

fn stop_with(cell: &StopCell, reason: Stop) {
    cell.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert(reason);
}

fn stopped(cell: &StopCell) -> Option<Stop> {
    *cell.lock().unwrap_or_else(|e| e.into_inner())
}

/// The prefix one stream kept, with the budget credit that retention holds.
/// Dropping it returns the credit.
#[derive(Default)]
struct Captured {
    bytes: Vec<u8>,
    _credit: Vec<Permit>,
}

/// Read a pipe to its end, keeping at most `keep` bytes. Bytes beyond that
/// are only counted, to decide whether a limit was crossed. Reading stops as
/// soon as any stream has stopped the command.
fn capture(
    mut pipe: impl Read,
    keep: usize,
    requested: Option<usize>,
    ceiling: usize,
    results: Arc<ByteBudget>,
    stop: StopCell,
) -> Captured {
    let mut captured = Captured::default();
    let mut seen = 0usize;
    let mut chunk = vec![0u8; STREAM_BLOCK];
    while stopped(&stop).is_none() {
        let read = match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        seen += read;
        if requested.is_some_and(|limit| seen > limit) {
            stop_with(&stop, Stop::Requested);
        } else if seen > ceiling {
            stop_with(&stop, Stop::Ceiling);
        }
        let take = keep.saturating_sub(captured.bytes.len()).min(read);
        if take > 0 {
            match results.try_acquire(take) {
                Some(permit) => {
                    captured._credit.push(permit);
                    captured.bytes.extend_from_slice(&chunk[..take]);
                }
                None => stop_with(&stop, Stop::Budget),
            }
        }
    }
    captured
}

/// Stop a child and its process group, so hooks and helpers started by git
/// stop too and the output pipes reach end-of-file. A limit stops at once. A
/// cancellation gets a short grace period to exit cleanly first.
fn terminate(child: &mut Child, grace: bool) {
    #[cfg(unix)]
    {
        fn signal_group(flag: &str, pid: u32) {
            let _ = Command::new("kill").args([flag, "--", &format!("-{pid}")]).status();
        }
        if grace {
            signal_group("-TERM", child.id());
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while std::time::Instant::now() < deadline {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        // Also reached after a clean exit, to remove stragglers in the group.
        signal_group("-KILL", child.id());
    }
    #[cfg(not(unix))]
    {
        let _ = grace;
        let _ = child.kill();
    }
}

/// The reason-carrying error for a stopped command. `partial` keeps the output
/// each stream produced, so callers can use what arrived (for example a
/// preview cut at `maxBuffer`).
fn stopped_error(stop: Stop, out: &Captured, err: &Captured, binary: bool, ceiling: usize) -> AppError {
    let encode = |bytes: &[u8]| {
        if binary {
            base64_encode(bytes)
        } else {
            String::from_utf8_lossy(bytes).into_owned()
        }
    };
    let encoding = if binary { "buffer" } else { "utf8" };
    match stop {
        Stop::Requested => AppError::Detailed {
            code: "ERR_CHILD_PROCESS_STDIO_MAXBUFFER".into(),
            message: "stdout maxBuffer length exceeded".into(),
            details: json!({
                "stdout": encode(&out.bytes),
                "stderr": encode(&err.bytes),
                "encoding": encoding,
                "truncated": true,
            }),
        },
        Stop::Ceiling => AppError::Detailed {
            code: "DESKTOP_GIT_OUTPUT_LIMIT".into(),
            message: format!(
                "git wrote more than {ceiling} bytes; the command was stopped and its output is not complete"
            ),
            // Output past the ceiling is never retained, so none is sent.
            details: json!({ "stdout": "", "stderr": "", "encoding": encoding, "truncated": true }),
        },
        Stop::Budget => AppError::Coded {
            code: "EBUSY".into(),
            message: "EBUSY: QUEUE_FULL, too much command output is being retained".into(),
        },
        Stop::Cancelled => aborted(),
    }
}

fn aborted() -> AppError {
    AppError::Coded {
        code: "ABORT_ERR".into(),
        message: "The operation was aborted.".into(),
    }
}

// ---------------------------------------------------------------------------
// cancellation
// ---------------------------------------------------------------------------

/// Cancellation flags by request id. A flag can be raised before its request
/// starts (the request is then refused) or while it runs (it is stopped).
fn cancel_flags() -> &'static Mutex<HashMap<u64, Arc<AtomicBool>>> {
    static FLAGS: OnceLock<Mutex<HashMap<u64, Arc<AtomicBool>>>> = OnceLock::new();
    FLAGS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Above this many flags, ones nobody is waiting on are dropped. A raised
/// flag is kept until its request has run.
const CANCEL_FLAG_LIMIT: usize = 1024;

fn cancel_flag(id: u64) -> Arc<AtomicBool> {
    let mut flags = cancel_flags().lock().unwrap_or_else(|e| e.into_inner());
    if flags.len() >= CANCEL_FLAG_LIMIT {
        flags.retain(|_, flag| Arc::strong_count(flag) > 1 || flag.load(Ordering::SeqCst));
    }
    flags.entry(id).or_insert_with(|| Arc::new(AtomicBool::new(false))).clone()
}

/// Holds a request's flag for the life of the request and removes it on drop.
struct CancelGuard {
    id: Option<u64>,
    flag: Option<Arc<AtomicBool>>,
}

impl CancelGuard {
    fn new(o: &Value) -> Self {
        let id = o.get("cancelId").and_then(Value::as_u64);
        Self { id, flag: id.map(cancel_flag) }
    }

    fn raised(&self) -> bool {
        self.flag.as_ref().is_some_and(|flag| flag.load(Ordering::SeqCst))
    }
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        if let Some(id) = self.id {
            cancel_flags().lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        }
    }
}

/// `desktop.io.cancel`: raise the flag of a request, queued or running.
fn cancel(o: &Value) -> Result<Value> {
    let id = unsigned(o, "cancelId")?;
    cancel_flag(id).store(true, Ordering::SeqCst);
    Ok(Value::Null)
}

fn unsigned(o: &Value, key: &str) -> Result<u64> {
    o.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| AppError::BadArgs(format!("`{key}` must be a non-negative integer")))
}

// ---------------------------------------------------------------------------
// git
// ---------------------------------------------------------------------------

/// Desktop points git at its credential trampoline through this pair of
/// config parameters. The trampoline needs a socket server inside the
/// renderer, which a webview cannot host, so the pair is removed and git
/// falls back to the user's own credential helpers.
const TRAMPOLINE_CREDENTIAL_CONFIG: &str = "'credential.helper=' 'credential.helper=desktop'";

/// `dugite.exec`: run git and resolve with its output whatever the exit code.
///
/// Output is read while git runs. `maxBuffer` (per stream, requested by the
/// caller) and the per-command ceiling both stop the command; neither is
/// reported as success. A stopped command's error carries the output it had
/// produced (`details`), up to `maxBuffer`.
fn git_exec(binary: &Path, resources: &Resources, o: &Value) -> Result<Value> {
    let args: Vec<String> = o
        .get("args")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .ok_or_else(|| AppError::BadArgs("missing `args`".into()))?;
    let cwd = string(o, "cwd").unwrap_or_default();
    let cancel = CancelGuard::new(o);
    if cancel.raised() {
        // Cancelled while queued: never start the command.
        return Err(aborted());
    }

    let mut command = Command::new(binary);
    command
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Never let git wait on a terminal the user cannot see.
        .env("GIT_TERMINAL_PROMPT", "0")
        // The renderer parses git's messages, which Desktop's bundled git
        // only ever printed in English. A system git is localized; this
        // selects the untranslated messages without changing the character
        // encoding the way LC_ALL=C would.
        .env("LANGUAGE", "en");
    if !cwd.is_empty() {
        command.current_dir(&cwd);
    }

    for (key, value) in git_environment(o.get("env")) {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }

    let stdin = match o.get("stdin").and_then(Value::as_str) {
        Some(encoded) => Some(base64_decode(encoded)?),
        None => None,
    };
    command.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so a stop reaches the hooks git starts.
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = command.spawn().map_err(|error| {
        // A missing working directory and a missing binary both surface as
        // NotFound; dugite reports either as ENOENT.
        io_error(&error, &format!("spawn git in {cwd:?}"))
    })?;

    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        // Write from a separate thread so a child that fills its stdout pipe
        // before draining stdin cannot deadlock us. A child that stops reading
        // ends this write with an error, which is ignored.
        std::thread::spawn(move || {
            let _ = pipe.write_all(&input);
        });
    }

    let requested = o.get("maxBuffer").and_then(Value::as_u64).map(|limit| limit as usize);
    let ceiling = resources.limits.git_output;
    let keep = requested.unwrap_or(usize::MAX).min(ceiling);
    let stop: StopCell = Arc::new(Mutex::new(None));
    let spawn_capture = |pipe: Option<Box<dyn Read + Send>>| {
        let stop = stop.clone();
        let results = resources.results.clone();
        pipe.map(|pipe| {
            std::thread::spawn(move || capture(pipe, keep, requested, ceiling, results, stop))
        })
    };
    let stdout_reader = spawn_capture(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let stderr_reader = spawn_capture(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));

    // Poll with a bounded, growing delay. Output is read on its own threads,
    // so this loop only decides when to stop the command.
    let mut delay = Duration::from_millis(1);
    loop {
        if cancel.raised() {
            stop_with(&stop, Stop::Cancelled);
        }
        if let Some(reason) = stopped(&stop) {
            terminate(&mut child, reason == Stop::Cancelled);
            break;
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                std::thread::sleep(delay);
                delay = (delay * 2).min(Duration::from_millis(20));
            }
            Err(_) => break,
        }
    }
    let status = child.wait();

    let out = stdout_reader.and_then(|h| h.join().ok()).unwrap_or_default();
    let err = stderr_reader.and_then(|h| h.join().ok()).unwrap_or_default();
    let binary_output = o.get("encoding").and_then(Value::as_str) == Some("buffer");
    if let Some(reason) = stopped(&stop) {
        return Err(stopped_error(reason, &out, &err, binary_output, ceiling));
    }

    let encode = |bytes: &[u8]| {
        if binary_output {
            base64_encode(bytes)
        } else {
            String::from_utf8_lossy(bytes).into_owned()
        }
    };
    Ok(json!({
        "stdout": encode(&out.bytes),
        "stderr": encode(&err.bytes),
        // Node reports a signal-terminated child with a null code; dugite
        // callers only compare against success codes.
        "exitCode": status.ok().and_then(|s| s.code()).unwrap_or(-1),
    }))
}

/// Translate the renderer's environment for a git child. `None` removes a
/// variable (the renderer passes `undefined` for those).
fn git_environment(env: Option<&Value>) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    let Some(map) = env.and_then(Value::as_object) else {
        return out;
    };

    for (key, value) in map {
        let value = match value {
            Value::String(s) => Some(s.clone()),
            Value::Null => None,
            other => Some(other.to_string()),
        };

        match key.as_str() {
            // These only make sense with the trampoline server running.
            "DESKTOP_PORT" | "DESKTOP_TRAMPOLINE_TOKEN" | "GIT_ASKPASS" | "SSH_ASKPASS" => {}
            // Bundled-git locations inside the Electron app directory.
            "LOCAL_GIT_DIRECTORY" | "GIT_EXEC_PATH" | "GIT_TEMPLATE_DIR" => {}
            "GIT_CONFIG_PARAMETERS" => {
                let cleaned = value
                    .unwrap_or_default()
                    .replace(TRAMPOLINE_CREDENTIAL_CONFIG, "")
                    .trim()
                    .to_string();
                out.push((key.clone(), (!cleaned.is_empty()).then_some(cleaned)));
            }
            _ => out.push((key.clone(), value)),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// http
// ---------------------------------------------------------------------------

fn http_client(follow_redirects: bool) -> &'static reqwest::blocking::Client {
    static FOLLOW: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    static MANUAL: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    let build = |policy| {
        reqwest::blocking::Client::builder()
            .redirect(policy)
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(300))
            .build()
            .expect("the HTTP client configuration is static")
    };
    if follow_redirects {
        FOLLOW.get_or_init(|| build(reqwest::redirect::Policy::limited(20)))
    } else {
        MANUAL.get_or_init(|| build(reqwest::redirect::Policy::none()))
    }
}

/// `fetch` for cross-origin requests. The page's origin is `desktop://`, so
/// the webview's own network stack applies CORS to every GitHub request;
/// Electron's renderer was exempt from it.
fn http_fetch(resources: &Resources, o: &Value) -> Result<Value> {
    let url = string(o, "url")?;
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(AppError::BadArgs("only http(s) URLs can be fetched".into()));
    }
    let method = reqwest::Method::from_bytes(
        o.get("method").and_then(Value::as_str).unwrap_or("GET").to_ascii_uppercase().as_bytes(),
    )
    .map_err(|_| AppError::BadArgs("invalid HTTP method".into()))?;
    let redirect = o.get("redirect").and_then(Value::as_str).unwrap_or("follow");

    let mut request = http_client(redirect == "follow").request(method, &url);
    if let Some(headers) = o.get("headers").and_then(Value::as_array) {
        for pair in headers {
            if let (Some(name), Some(value)) = (
                pair.get(0).and_then(Value::as_str),
                pair.get(1).and_then(Value::as_str),
            ) {
                request = request.header(name, value);
            }
        }
    }
    if let Some(body) = o.get("body").and_then(Value::as_str) {
        request = request.body(base64_decode(body)?);
    }

    let response = request.send()?;
    let status = response.status();
    if redirect == "error" && status.is_redirection() {
        return Err(AppError::msg("the request was redirected, which it does not allow"));
    }

    let headers: Vec<Value> = response
        .headers()
        .iter()
        .map(|(name, value)| json!([name.as_str(), String::from_utf8_lossy(value.as_bytes())]))
        .collect();
    let final_url = response.url().to_string();
    // Read at most the ceiling plus one byte, so an oversized body is detected
    // without being retained.
    let ceiling = resources.limits.http_response;
    let mut body = Vec::new();
    response.take(ceiling as u64 + 1).read_to_end(&mut body)?;
    if body.len() > ceiling {
        return Err(AppError::Detailed {
            code: "DESKTOP_HTTP_RESPONSE_LIMIT".into(),
            message: format!("the response is larger than {ceiling} bytes and was not retained"),
            details: json!({ "truncated": true }),
        });
    }
    let _credit = resources
        .results
        .try_acquire(body.len())
        .ok_or_else(|| AppError::Coded {
            code: "EBUSY".into(),
            message: "EBUSY: QUEUE_FULL, too many results are being retained".into(),
        })?;

    Ok(json!({
        "url": final_url,
        "status": status.as_u16(),
        "statusText": status.canonical_reason().unwrap_or(""),
        "headers": headers,
        "body": base64_encode(&body),
    }))
}

// ---------------------------------------------------------------------------
// fs
// ---------------------------------------------------------------------------

/// Map an I/O failure onto the `code` Node attaches to its errors; renderer
/// code branches on `ENOENT` and friends.
fn io_error(error: &std::io::Error, context: &str) -> AppError {
    use std::io::ErrorKind as K;
    let code = match error.kind() {
        K::NotFound => "ENOENT",
        K::PermissionDenied => "EACCES",
        K::AlreadyExists => "EEXIST",
        K::NotADirectory => "ENOTDIR",
        K::IsADirectory => "EISDIR",
        K::DirectoryNotEmpty => "ENOTEMPTY",
        K::InvalidInput => "EINVAL",
        K::CrossesDevices => "EXDEV",
        K::ReadOnlyFilesystem => "EROFS",
        K::StorageFull => "ENOSPC",
        K::TimedOut => "ETIMEDOUT",
        K::Interrupted => "EINTR",
        _ => "EIO",
    };
    AppError::Coded {
        code: code.to_string(),
        message: format!("{code}: {error}, {context}"),
    }
}

fn metadata_json(meta: &std::fs::Metadata) -> Value {
    let millis = |time: std::io::Result<std::time::SystemTime>| {
        time.ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64() * 1000.0)
            .unwrap_or(0.0)
    };
    #[cfg(unix)]
    let (mode, dev, ino) = {
        use std::os::unix::fs::MetadataExt;
        (meta.mode(), meta.dev(), meta.ino())
    };
    #[cfg(not(unix))]
    let (mode, dev, ino) = (if meta.permissions().readonly() { 0o444u32 } else { 0o666u32 }, 0u64, 0u64);

    let file_type = meta.file_type();
    json!({
        "isFile": file_type.is_file(),
        "isDirectory": file_type.is_dir(),
        "isSymbolicLink": file_type.is_symlink(),
        "size": meta.len(),
        "mode": mode,
        "dev": dev,
        // Inode numbers can exceed 2^53; identity comparisons need the exact value.
        "ino": ino.to_string(),
        "mtimeMs": millis(meta.modified()),
        "atimeMs": millis(meta.accessed()),
        "birthtimeMs": millis(meta.created()),
        "ctimeMs": millis(meta.modified()),
    })
}

fn fs_invoke(operation: &str, o: &Value) -> Result<Value> {
    let path = || string(o, "path").map(PathBuf::from);
    let wrap = |result: std::io::Result<Value>, what: &str| {
        result.map_err(|e| io_error(&e, &format!("{what} '{}'", string(o, "path").unwrap_or_default())))
    };

    match operation {
        "read-file" => wrap(std::fs::read(path()?).map(|b| json!(base64_encode(&b))), "open"),
        "write-file" => {
            let data = base64_decode(&string(o, "data")?)?;
            let target = path()?;
            wrap(
                (|| {
                    let mut options = std::fs::OpenOptions::new();
                    options.write(true);
                    if flag(o, "exclusive") {
                        // O_CREAT | O_EXCL: fail with EEXIST rather than overwrite.
                        options.create_new(true);
                    } else {
                        options.create(true);
                    }
                    if flag(o, "exclusive") {
                    } else if flag(o, "append") {
                        options.append(true);
                    } else {
                        options.truncate(true);
                    }
                    #[cfg(unix)]
                    if let Some(mode) = o.get("mode").and_then(Value::as_u64) {
                        use std::os::unix::fs::OpenOptionsExt;
                        options.mode(mode as u32);
                    }
                    options.open(&target)?.write_all(&data)?;
                    Ok(Value::Null)
                })(),
                "open",
            )
        }
        "stat" => wrap(std::fs::metadata(path()?).map(|m| metadata_json(&m)), "stat"),
        "lstat" => wrap(std::fs::symlink_metadata(path()?).map(|m| metadata_json(&m)), "lstat"),
        "exists" => Ok(json!(path()?.exists())),
        "access" => {
            let target = path()?;
            let meta = wrap(std::fs::metadata(&target).map(|_| Value::Null), "access")?;
            // W_OK = 2. A read-only file fails a write check.
            if o.get("mode").and_then(Value::as_u64).unwrap_or(0) & 2 != 0
                && std::fs::metadata(&target).map(|m| m.permissions().readonly()).unwrap_or(false)
            {
                return Err(AppError::Coded {
                    code: "EACCES".into(),
                    message: format!("EACCES: permission denied, access '{}'", target.display()),
                });
            }
            Ok(meta)
        }
        "readdir" => {
            let target = path()?;
            wrap(
                (|| {
                    let mut entries = Vec::new();
                    for entry in std::fs::read_dir(&target)? {
                        let entry = entry?;
                        let file_type = entry.file_type()?;
                        entries.push(json!({
                            "name": entry.file_name().to_string_lossy(),
                            "isFile": file_type.is_file(),
                            "isDirectory": file_type.is_dir(),
                            "isSymbolicLink": file_type.is_symlink(),
                        }));
                    }
                    Ok(Value::Array(entries))
                })(),
                "scandir",
            )
        }
        "mkdir" => {
            let target = path()?;
            let result = if flag(o, "recursive") {
                std::fs::create_dir_all(&target)
            } else {
                std::fs::create_dir(&target)
            };
            wrap(result.map(|_| Value::Null), "mkdir")
        }
        "mkdtemp" => {
            let prefix = string(o, "prefix")?;
            for attempt in 0..32u32 {
                let suffix = unique_suffix(attempt);
                let candidate = PathBuf::from(format!("{prefix}{suffix}"));
                match std::fs::create_dir(&candidate) {
                    Ok(()) => return Ok(json!(candidate.to_string_lossy())),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(e) => return Err(io_error(&e, &format!("mkdtemp '{prefix}XXXXXX'"))),
                }
            }
            Err(AppError::msg("unable to create a unique temporary directory"))
        }
        "rm" => {
            let target = path()?;
            let result = match std::fs::symlink_metadata(&target) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && flag(o, "force") => Ok(()),
                Err(e) => Err(e),
                Ok(meta) if meta.is_dir() && flag(o, "recursive") => std::fs::remove_dir_all(&target),
                Ok(meta) if meta.is_dir() => std::fs::remove_dir(&target),
                Ok(_) => std::fs::remove_file(&target),
            };
            wrap(result.map(|_| Value::Null), "rm")
        }
        "unlink" => wrap(std::fs::remove_file(path()?).map(|_| Value::Null), "unlink"),
        "rmdir" => wrap(std::fs::remove_dir(path()?).map(|_| Value::Null), "rmdir"),
        "rename" => wrap(
            std::fs::rename(path()?, string(o, "to")?).map(|_| Value::Null),
            "rename",
        ),
        "copy-file" => wrap(
            std::fs::copy(path()?, string(o, "to")?).map(|_| Value::Null),
            "copyfile",
        ),
        "copy" => wrap(copy_recursive(&path()?, Path::new(&string(o, "to")?)).map(|_| Value::Null), "cp"),
        "realpath" => wrap(
            std::fs::canonicalize(path()?).map(|p| json!(p.to_string_lossy())),
            "realpath",
        ),
        "readlink" => wrap(
            std::fs::read_link(path()?).map(|p| json!(p.to_string_lossy())),
            "readlink",
        ),
        "symlink" => {
            let link = path()?;
            let target = string(o, "target")?;
            #[cfg(unix)]
            let result = std::os::unix::fs::symlink(&target, &link);
            #[cfg(windows)]
            let result = if Path::new(&target).is_dir() {
                std::os::windows::fs::symlink_dir(&target, &link)
            } else {
                std::os::windows::fs::symlink_file(&target, &link)
            };
            wrap(result.map(|_| Value::Null), "symlink")
        }
        "chmod" => {
            let target = path()?;
            #[cfg(unix)]
            let result = {
                use std::os::unix::fs::PermissionsExt;
                let mode = o.get("mode").and_then(Value::as_u64).unwrap_or(0o644) as u32;
                std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode))
            };
            #[cfg(not(unix))]
            let result = std::fs::metadata(&target).map(|_| ());
            wrap(result.map(|_| Value::Null), "chmod")
        }
        other => Err(AppError::UnknownChannel(format!("{PREFIX}fs-{other}"))),
    }
}

/// Largest range one `fs-read-range` call returns. Longer reads are chained.
const RANGE_LIMIT: usize = STREAM_BLOCK;

/// `fs-read-range`: `length` bytes at `offset`, read with a seek. Returns the
/// file identity too, so a caller reading in blocks can detect a replaced or
/// changed file and fail instead of mixing versions.
fn read_range(o: &Value) -> Result<Value> {
    let path = PathBuf::from(string(o, "path")?);
    let offset = unsigned(o, "offset")?;
    let length = unsigned(o, "length")?;
    if length > RANGE_LIMIT as u64 {
        return Err(AppError::BadArgs(format!("`length` may not exceed {RANGE_LIMIT} bytes")));
    }
    offset
        .checked_add(length)
        .ok_or_else(|| AppError::BadArgs("`offset` + `length` overflows".into()))?;

    let mut file = File::open(&path).map_err(|e| io_error(&e, &format!("open '{}'", path.display())))?;
    let meta = file.metadata().map_err(|e| io_error(&e, &format!("stat '{}'", path.display())))?;
    if meta.is_dir() {
        return Err(AppError::Coded {
            code: "EISDIR".into(),
            message: format!("EISDIR: illegal operation on a directory, read '{}'", path.display()),
        });
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut data = vec![0u8; length as usize];
    let mut filled = 0usize;
    while filled < data.len() {
        let read = file.read(&mut data[filled..])?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    data.truncate(filled);
    Ok(json!({
        "data": base64_encode(&data),
        "bytesRead": filled,
        "eof": offset.saturating_add(filled as u64) >= meta.len(),
        "identity": metadata_json(&meta),
    }))
}

/// `fs-hash`: the hex digest of a file, computed in blocks here. The file
/// never crosses to the renderer. Honours `cancelId` between blocks.
fn file_hash(o: &Value) -> Result<Value> {
    let algorithm = string(o, "algorithm")?;
    let path = PathBuf::from(string(o, "path")?);
    let cancel = CancelGuard::new(o);
    if cancel.raised() {
        return Err(aborted());
    }
    let mut file = File::open(&path).map_err(|e| io_error(&e, &format!("open '{}'", path.display())))?;
    if file.metadata().map(|m| m.is_dir()).unwrap_or(false) {
        return Err(AppError::Coded {
            code: "EISDIR".into(),
            message: format!("EISDIR: illegal operation on a directory, read '{}'", path.display()),
        });
    }
    let digest = match algorithm.as_str() {
        "sha1" => hash_blocks(&mut file, sha1::Sha1::new(), &cancel)?,
        "sha256" => hash_blocks(&mut file, sha2::Sha256::new(), &cancel)?,
        other => return Err(AppError::BadArgs(format!("unsupported digest `{other}`"))),
    };
    Ok(json!(digest))
}

fn hash_blocks<D: sha2::Digest>(file: &mut File, mut hasher: D, cancel: &CancelGuard) -> Result<String> {
    let mut chunk = vec![0u8; STREAM_BLOCK];
    loop {
        if cancel.raised() {
            return Err(aborted());
        }
        let read = file.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        hasher.update(&chunk[..read]);
    }
    Ok(hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
}

fn copy_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

/// Six characters for `mkdtemp`, unique per process, call and attempt.
fn unique_suffix(attempt: u32) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

    let nanos = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut seed = nanos
        ^ (std::process::id() as u64).rotate_left(32)
        ^ COUNTER.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ attempt as u64;
    (0..6)
        .map(|_| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ALPHABET[((seed >> 33) % ALPHABET.len() as u64) as usize] as char
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::limits::{Limits, Profile};

    fn test_resources() -> Resources {
        Resources::new(Limits::for_profile(Profile::Default))
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("desktop-io-{name}-{}", unique_suffix(0)));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn code(error: AppError) -> String {
        error.to_rpc_error().code.unwrap_or_default()
    }

    #[test]
    fn fs_round_trip_and_node_error_codes() {
        let dir = temp_dir("fs");
        let file = dir.join("a.bin");
        let path = json!({ "path": file, "data": base64_encode(&[0, 159, 255, 10]) });

        fs_invoke("write-file", &path).unwrap();
        assert_eq!(fs_invoke("read-file", &path).unwrap(), json!(base64_encode(&[0, 159, 255, 10])));

        let stat = fs_invoke("stat", &path).unwrap();
        assert_eq!(stat["isFile"], true);
        assert_eq!(stat["size"], 4);

        let listing = fs_invoke("readdir", &json!({ "path": dir })).unwrap();
        assert_eq!(listing[0]["name"], "a.bin");

        let missing = json!({ "path": dir.join("missing") });
        assert_eq!(code(fs_invoke("stat", &missing).unwrap_err()), "ENOENT");
        assert_eq!(code(fs_invoke("read-file", &missing).unwrap_err()), "ENOENT");
        assert_eq!(fs_invoke("exists", &missing).unwrap(), json!(false));
        assert_eq!(code(fs_invoke("mkdir", &json!({ "path": dir })).unwrap_err()), "EEXIST");
        let exclusive = json!({ "path": file, "data": "", "exclusive": true });
        assert_eq!(code(fs_invoke("write-file", &exclusive).unwrap_err()), "EEXIST");
        assert_eq!(code(fs_invoke("rm", &json!({ "path": dir })).unwrap_err()), "ENOTEMPTY");
        fs_invoke("rm", &json!({ "path": dir.join("missing"), "force": true })).unwrap();

        fs_invoke("rm", &json!({ "path": dir, "recursive": true })).unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn mkdtemp_creates_distinct_directories() {
        let prefix = std::env::temp_dir().join("desktop-io-mkdtemp-");
        let o = json!({ "prefix": prefix });
        let a = fs_invoke("mkdtemp", &o).unwrap();
        let b = fs_invoke("mkdtemp", &o).unwrap();
        assert_ne!(a, b);
        for dir in [a, b] {
            let dir = dir.as_str().unwrap();
            assert!(Path::new(dir).is_dir());
            std::fs::remove_dir(dir).unwrap();
        }
    }

    #[test]
    fn git_exec_reports_output_and_exit_code_without_failing() {
        let ok = git_exec(Path::new("git"), &test_resources(), &json!({ "args": ["--version"] })).unwrap();
        assert_eq!(ok["exitCode"], 0);
        assert!(ok["stdout"].as_str().unwrap().starts_with("git version"));

        // A failing command resolves; the renderer decides what is an error.
        let failed = git_exec(Path::new("git"), &test_resources(), &json!({ "args": ["not-a-command"] })).unwrap();
        assert_ne!(failed["exitCode"], 0);
        assert!(!failed["stderr"].as_str().unwrap().is_empty());
    }

    #[test]
    fn git_exec_passes_stdin_and_returns_binary_output() {
        let dir = temp_dir("git");
        let o = json!({
            "args": ["hash-object", "--stdin"],
            "cwd": dir,
            "stdin": base64_encode(b"hello\n"),
            "encoding": "buffer",
        });
        let out = git_exec(Path::new("git"), &test_resources(), &o).unwrap();
        let stdout = base64_decode(out["stdout"].as_str().unwrap()).unwrap();
        assert_eq!(String::from_utf8(stdout).unwrap().trim(), "ce013625030ba8dba906f756967f9e9ca394464a");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn git_exec_output_is_english_whatever_the_user_locale() {
        // Fails only if a translation would otherwise apply; harmless elsewhere.
        let out = git_exec(
            Path::new("git"),
            &test_resources(),
            &json!({ "args": ["definitely-not-a-command"], "env": { "LANG": "es_ES.UTF-8", "LC_ALL": null } }),
        )
        .unwrap();
        assert!(out["stderr"].as_str().unwrap().contains("is not a git command"));
    }

    #[test]
    fn git_exec_missing_directory_is_enoent() {
        let o = json!({ "args": ["status"], "cwd": "/definitely/not/a/real/directory" });
        assert_eq!(code(git_exec(Path::new("git"), &test_resources(), &o).unwrap_err()), "ENOENT");
    }

    #[test]
    fn git_exec_enforces_max_buffer() {
        let o = json!({ "args": ["--version"], "maxBuffer": 2 });
        assert_eq!(
            code(git_exec(Path::new("git"), &test_resources(), &o).unwrap_err()),
            "ERR_CHILD_PROCESS_STDIO_MAXBUFFER"
        );
    }

    #[test]
    fn trampoline_settings_are_removed_from_the_git_environment() {
        let env = json!({
            "DESKTOP_PORT": 0,
            "DESKTOP_TRAMPOLINE_TOKEN": "t",
            "GIT_ASKPASS": "",
            "LOCAL_GIT_DIRECTORY": "/__desktop_app__/git",
            "TERM": "dumb",
            "GIT_EDITOR": null,
            "GIT_CONFIG_PARAMETERS": "'a.b=c' 'credential.helper=' 'credential.helper=desktop'",
        });
        let mut out = git_environment(Some(&env));
        out.sort();
        assert_eq!(
            out,
            vec![
                ("GIT_CONFIG_PARAMETERS".to_string(), Some("'a.b=c'".to_string())),
                ("GIT_EDITOR".to_string(), None),
                ("TERM".to_string(), Some("dumb".to_string())),
            ]
        );

        let only_trampoline = json!({ "GIT_CONFIG_PARAMETERS": TRAMPOLINE_CREDENTIAL_CONFIG });
        assert_eq!(
            git_environment(Some(&only_trampoline)),
            vec![("GIT_CONFIG_PARAMETERS".to_string(), None)]
        );
    }

    fn code_and_details(error: AppError) -> (String, Option<Value>) {
        let rpc = error.to_rpc_error();
        (rpc.code.unwrap_or_default(), rpc.details)
    }

    #[test]
    fn max_buffer_error_keeps_the_prefix_that_was_produced() {
        // `git --version` prints one line; a 5-byte limit must keep exactly 5 bytes.
        let o = json!({ "args": ["--version"], "maxBuffer": 5 });
        let (code, details) = code_and_details(git_exec(Path::new("git"), &test_resources(), &o).unwrap_err());
        assert_eq!(code, "ERR_CHILD_PROCESS_STDIO_MAXBUFFER");
        let details = details.expect("the error carries the captured output");
        assert_eq!(details["stdout"], "git v");
        assert_eq!(details["truncated"], true);
    }

    #[test]
    fn the_retention_ceiling_is_separate_from_maxbuffer_and_never_success() {
        let mut limits = Limits::for_profile(Profile::Default);
        limits.git_output = 8;
        let resources = Resources::new(limits);
        let o = json!({ "args": ["--version"] });
        let (code, details) = code_and_details(git_exec(Path::new("git"), &resources, &o).unwrap_err());
        assert_eq!(code, "DESKTOP_GIT_OUTPUT_LIMIT");
        assert_eq!(details.unwrap()["stdout"], "", "output past the ceiling is not sent");
    }

    #[test]
    fn a_stopped_command_releases_its_result_credit() {
        let mut limits = Limits::for_profile(Profile::Default);
        limits.git_output = 8;
        let resources = Resources::new(limits);
        let _ = git_exec(Path::new("git"), &resources, &json!({ "args": ["--version"] }));
        assert_eq!(resources.results.used(), 0);
    }

    #[test]
    fn a_cancel_raised_before_the_request_starts_prevents_it() {
        cancel(&json!({ "cancelId": 4_000_001u64 })).unwrap();
        let o = json!({ "args": ["--version"], "cancelId": 4_000_001u64 });
        let (code, _) = code_and_details(git_exec(Path::new("git"), &test_resources(), &o).unwrap_err());
        assert_eq!(code, "ABORT_ERR");
        assert!(cancel_flags().lock().unwrap().get(&4_000_001).is_none(), "the flag is released");
    }

    #[test]
    fn a_cancel_raised_while_running_stops_the_process_group() {
        let started = std::time::Instant::now();
        let canceller = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(200));
            cancel(&json!({ "cancelId": 4_000_002u64 })).unwrap();
        });
        let o = json!({ "args": ["-c", "sleep 30"], "cancelId": 4_000_002u64 });
        let (code, _) = code_and_details(git_exec(Path::new("sh"), &test_resources(), &o).unwrap_err());
        canceller.join().unwrap();
        assert_eq!(code, "ABORT_ERR");
        assert!(started.elapsed() < Duration::from_secs(10), "the sleeping child was stopped");
    }

    #[test]
    fn exceeding_maxbuffer_stops_a_child_that_keeps_writing() {
        let started = std::time::Instant::now();
        let o = json!({ "args": ["-c", "head -c 50000000 /dev/zero"], "maxBuffer": 1000, "encoding": "buffer" });
        let (code, details) = code_and_details(git_exec(Path::new("sh"), &test_resources(), &o).unwrap_err());
        assert_eq!(code, "ERR_CHILD_PROCESS_STDIO_MAXBUFFER");
        let details = details.unwrap();
        assert_eq!(base64_decode(details["stdout"].as_str().unwrap()).unwrap().len(), 1000);
        assert!(started.elapsed() < Duration::from_secs(20), "the writer was stopped, not waited for");
    }

    #[test]
    fn range_reads_return_exact_bytes_and_report_eof() {
        let dir = temp_dir("range");
        let file = dir.join("big.bin");
        let content: Vec<u8> = (0..(3 * 1024 * 1024 + 17)).map(|i| (i % 253) as u8).collect();
        std::fs::write(&file, &content).unwrap();

        let block = read_range(&json!({ "path": file, "offset": 1024 * 1024u64, "length": 65536u64 })).unwrap();
        let bytes = base64_decode(block["data"].as_str().unwrap()).unwrap();
        assert_eq!(block["bytesRead"], 65536);
        assert_eq!(block["eof"], false);
        assert_eq!(bytes, content[1024 * 1024..1024 * 1024 + 65536]);

        let tail = read_range(&json!({ "path": file, "offset": content.len() as u64 - 5, "length": 65536u64 })).unwrap();
        assert_eq!(tail["bytesRead"], 5);
        assert_eq!(tail["eof"], true);
        let past = read_range(&json!({ "path": file, "offset": content.len() as u64 + 10, "length": 8u64 })).unwrap();
        assert_eq!(past["bytesRead"], 0);
        assert_eq!(past["eof"], true);

        assert!(matches!(
            read_range(&json!({ "path": file, "offset": 0u64, "length": 65537u64 })),
            Err(AppError::BadArgs(_))
        ));
        assert!(matches!(
            read_range(&json!({ "path": file, "offset": u64::MAX, "length": 2u64 })),
            Err(AppError::BadArgs(_))
        ));
        assert!(matches!(
            read_range(&json!({ "path": file, "offset": -1, "length": 2u64 })),
            Err(AppError::BadArgs(_))
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn file_hashes_match_known_digests() {
        let dir = temp_dir("hash");
        let file = dir.join("abc.txt");
        std::fs::write(&file, b"abc").unwrap();
        assert_eq!(
            file_hash(&json!({ "path": file, "algorithm": "sha1" })).unwrap(),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            file_hash(&json!({ "path": file, "algorithm": "sha256" })).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(matches!(
            file_hash(&json!({ "path": file, "algorithm": "md5" })),
            Err(AppError::BadArgs(_))
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn http_fetch_rejects_non_http_urls() {
        let error = http_fetch(&test_resources(), &json!({ "url": "file:///etc/passwd" })).unwrap_err();
        assert!(matches!(error, AppError::BadArgs(_)));
    }
}
