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

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::app::DesktopApp;
use crate::error::{AppError, Result};
use crate::storage::{base64_decode, base64_encode};

pub const PREFIX: &str = "desktop.io.";

mod process;

pub use process::{EVENT as PROCESS_EVENT, PROCESS_OPERATIONS};

/// The renderer's `__dirname` in the web build (`BundleRoot` in
/// `app/src/web/bundle.ts`). Not a real directory: the shims map paths under
/// it onto resources served from the page origin.
pub const BUNDLE_ROOT: &str = "/__desktop_app__";

/// Whether `channel` is served by this module.
pub fn handles(channel: &str) -> bool {
    channel.starts_with(PREFIX)
}

pub fn invoke(app: &DesktopApp, channel: &str, args: &[Value]) -> Result<Value> {
    let empty = json!({});
    let o = args.first().unwrap_or(&empty);
    match channel.strip_prefix(PREFIX).unwrap_or("") {
        "git-exec" => git_exec(&app.git.binary, o),
        "http-fetch" => http_fetch(o),
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
        "process-kill" => process::kill(o),
        other => match other.strip_prefix("fs-") {
            Some(operation) => fs_invoke(operation, o),
            None => Err(AppError::UnknownChannel(channel.to_string())),
        },
    }
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

/// Desktop points git at its credential trampoline through this pair of
/// config parameters. The trampoline needs a socket server inside the
/// renderer, which a webview cannot host, so the pair is removed and git
/// falls back to the user's own credential helpers.
const TRAMPOLINE_CREDENTIAL_CONFIG: &str = "'credential.helper=' 'credential.helper=desktop'";

/// `dugite.exec`: run git and resolve with its output whatever the exit code.
fn git_exec(binary: &Path, o: &Value) -> Result<Value> {
    let args: Vec<String> = o
        .get("args")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .ok_or_else(|| AppError::BadArgs("missing `args`".into()))?;
    let cwd = string(o, "cwd").unwrap_or_default();

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
        // before draining stdin cannot deadlock us.
        std::thread::spawn(move || {
            let _ = pipe.write_all(&input);
        });
    }

    let output = child.wait_with_output()?;

    let max_buffer = o.get("maxBuffer").and_then(Value::as_u64);
    if let Some(limit) = max_buffer {
        if output.stdout.len() as u64 > limit || output.stderr.len() as u64 > limit {
            return Err(AppError::Coded {
                code: "ERR_CHILD_PROCESS_STDIO_MAXBUFFER".into(),
                message: "stdout maxBuffer length exceeded".into(),
            });
        }
    }

    let binary_output = o.get("encoding").and_then(Value::as_str) == Some("buffer");
    let encode = |bytes: &[u8]| {
        if binary_output {
            base64_encode(bytes)
        } else {
            String::from_utf8_lossy(bytes).into_owned()
        }
    };

    Ok(json!({
        "stdout": encode(&output.stdout),
        "stderr": encode(&output.stderr),
        // Node reports a signal-terminated child with a null code; dugite
        // callers only compare against success codes.
        "exitCode": output.status.code().unwrap_or(-1),
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
fn http_fetch(o: &Value) -> Result<Value> {
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
    let body = response.bytes()?;

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
        let ok = git_exec(Path::new("git"), &json!({ "args": ["--version"] })).unwrap();
        assert_eq!(ok["exitCode"], 0);
        assert!(ok["stdout"].as_str().unwrap().starts_with("git version"));

        // A failing command resolves; the renderer decides what is an error.
        let failed = git_exec(Path::new("git"), &json!({ "args": ["not-a-command"] })).unwrap();
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
        let out = git_exec(Path::new("git"), &o).unwrap();
        let stdout = base64_decode(out["stdout"].as_str().unwrap()).unwrap();
        assert_eq!(String::from_utf8(stdout).unwrap().trim(), "ce013625030ba8dba906f756967f9e9ca394464a");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn git_exec_output_is_english_whatever_the_user_locale() {
        // Fails only if a translation would otherwise apply; harmless elsewhere.
        let out = git_exec(
            Path::new("git"),
            &json!({ "args": ["definitely-not-a-command"], "env": { "LANG": "es_ES.UTF-8", "LC_ALL": null } }),
        )
        .unwrap();
        assert!(out["stderr"].as_str().unwrap().contains("is not a git command"));
    }

    #[test]
    fn git_exec_missing_directory_is_enoent() {
        let o = json!({ "args": ["status"], "cwd": "/definitely/not/a/real/directory" });
        assert_eq!(code(git_exec(Path::new("git"), &o).unwrap_err()), "ENOENT");
    }

    #[test]
    fn git_exec_enforces_max_buffer() {
        let o = json!({ "args": ["--version"], "maxBuffer": 2 });
        assert_eq!(
            code(git_exec(Path::new("git"), &o).unwrap_err()),
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

    #[test]
    fn http_fetch_rejects_non_http_urls() {
        let error = http_fetch(&json!({ "url": "file:///etc/passwd" })).unwrap_err();
        assert!(matches!(error, AppError::BadArgs(_)));
    }
}
