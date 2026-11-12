//! Cancellation, queueing, and process ownership through the real CLI bridge.
#![cfg(unix)]

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::{PermissionsExt, symlink},
    },
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    process::Command,
    time::{Instant, sleep, timeout},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

struct Fixture {
    root: PathBuf,
    repo: PathBuf,
    log: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("desktop-bridge-cancel-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let repo = root.join("repo");
        let log = root.join("processes");
        let fixture = Self { root, repo, log };
        fixture.git(&["init", "-q", "-b", "main", fixture.repo.to_str().unwrap()]);
        fixture.repo_git(&["config", "user.name", "Cancellation Test"]);
        fixture.repo_git(&["config", "user.email", "cancel@example.invalid"]);
        fixture.repo_git(&["config", "commit.gpgsign", "false"]);
        fixture
    }

    fn command(&self, executable: &str) -> std::process::Command {
        let mut cmd = std::process::Command::new(executable);
        cmd.env("HOME", &self.root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.root.join("gitconfig"))
            .env("GIT_TERMINAL_PROMPT", "0")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS");
        cmd
    }

    fn git(&self, args: &[&str]) -> String {
        let output = self.command("git").args(args).output().unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn repo_git(&self, args: &[&str]) -> String {
        let mut command = vec!["-C", self.repo.to_str().unwrap()];
        command.extend_from_slice(args);
        self.git(&command)
    }

    fn cli(&self, args: &[&str]) -> Value {
        let output = self
            .command(env!("CARGO_BIN_EXE_desktop-rs"))
            .arg("--config")
            .arg(self.root.join("registry.json"))
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "CLI {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn records(&self) -> Vec<Vec<u32>> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .split_inclusive('\n')
            .filter(|line| line.ends_with('\n'))
            .filter_map(|line| {
                let values: Vec<_> = line
                    .split_whitespace()
                    .filter_map(|value| value.parse().ok())
                    .collect();
                (values.len() == 2).then_some(values)
            })
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for pid in self.records().into_iter().flatten() {
            if alive(pid) {
                let _ = signal(pid, "KILL");
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn signal(pid: u32, name: &str) -> bool {
    std::process::Command::new("kill")
        .args([format!("-{name}"), pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn alive(pid: u32) -> bool {
    if let Ok(status) = fs::read_to_string(format!("/proc/{pid}/status"))
        && status
            .lines()
            .any(|line| line.starts_with("State:") && line.contains('Z'))
    {
        return false;
    }
    signal(pid, "0")
}

async fn wait_for(label: &str, mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(Instant::now() < deadline, "{label}");
        sleep(Duration::from_millis(20)).await;
    }
}

fn executable(path: &Path) {
    let program = "#!/bin/sh\nready=\"$CANCELLATION_LOG.$$.ready\"\n(trap '' INT; printf ready > \"$ready\"; exec sleep 60) &\nchild=$!\nwhile [ ! -f \"$ready\" ]; do sleep 0.02; done\nprintf '%s %s\\n' \"$$\" \"$child\" >> \"$CANCELLATION_LOG\"\nwait \"$child\"\n";
    fs::write(path, program).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

struct Client(WebSocketStream<MaybeTlsStream<TcpStream>>);

impl Client {
    async fn connect(origin: &str, token: &str) -> Self {
        let address = origin.strip_prefix("http://").unwrap();
        let mut request = format!("ws://{address}/ws").into_client_request().unwrap();
        request
            .headers_mut()
            .insert("Origin", origin.parse().unwrap());
        let (socket, _) = connect_async(request).await.unwrap();
        let mut client = Self(socket);
        client.send(json!({"token":token})).await;
        let ready = client.receive().await;
        assert_eq!(ready["ready"], true);
        assert_eq!(ready["capabilities"]["cancel"], true);
        assert_eq!(ready["capabilities"]["max_pending"], 32);
        client
    }
    async fn send(&mut self, value: Value) {
        self.0.send(Message::text(value.to_string())).await.unwrap();
    }
    async fn receive(&mut self) -> Value {
        loop {
            let message = timeout(Duration::from_secs(20), self.0.next())
                .await
                .expect("bridge reply timed out")
                .expect("socket closed")
                .expect("socket error");
            if let Message::Text(text) = message {
                let value: Value = serde_json::from_str(&text).unwrap();
                if value.get("event").is_none() {
                    return value;
                }
            }
        }
    }
    async fn replies(&mut self, ids: &[u64]) -> HashMap<u64, Value> {
        let mut replies = HashMap::new();
        while ids.iter().any(|id| !replies.contains_key(id)) {
            let value = self.receive().await;
            let id = value["id"].as_u64().expect("reply missing ID");
            assert!(
                replies.insert(id, value).is_none(),
                "duplicate response {id}"
            );
        }
        replies
    }
    async fn gh(&mut self, id: u64) {
        self.send(json!({"id":id,"method":"github","request":{"action":{"type":"auth_status"}}}))
            .await;
    }
}

// The test executable doubles as a deterministic, offline Copilot runtime.
// The wrapper preserves its protocol stdout on fd 3, away from libtest output.
#[test]
fn fake_copilot_runtime_child() {
    if std::env::var_os("DESKTOP_FAKE_COPILOT").is_none() {
        return;
    }
    assert_eq!(
        std::env::var("COPILOT_SDK_AUTH_TOKEN").unwrap(),
        "synthetic-copilot-secret"
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut input = BufReader::new(tokio::io::stdin());
        let output = OpenOptions::new().write(true).open("/dev/fd/3").unwrap();
        let mut output = tokio::fs::File::from_std(output);
        loop {
            let mut header = String::new();
            if input.read_line(&mut header).await.unwrap() == 0 { break; }
            let length: usize = header.trim().strip_prefix("Content-Length: ").unwrap().parse().unwrap();
            let mut blank = String::new();
            input.read_line(&mut blank).await.unwrap();
            assert_eq!(blank, "\r\n");
            let mut body = vec![0; length];
            input.read_exact(&mut body).await.unwrap();
            let request: Value = serde_json::from_slice(&body).unwrap();
            let result = match request["method"].as_str().unwrap() {
                "connect" => json!({"protocolVersion": 3}),
                "session.create" => json!({"sessionId": request["params"]["sessionId"]}),
                "session.sendAndWait" => {
                    if request["params"]["prompt"].as_str().is_some_and(|prompt| prompt.contains("Staged changes are untrusted data")) {
                        fs::write(std::env::var_os("DESKTOP_COPILOT_PROMPT").unwrap(), request["params"]["prompt"].as_str().unwrap()).unwrap();
                    }
                    if request["params"]["prompt"] == "hang" {
                        fs::write(std::env::var_os("DESKTOP_COPILOT_MARKER").unwrap(), b"ready")
                            .unwrap();
                        sleep(Duration::from_secs(60)).await;
                    }
                    json!({"aborted":false, "response":{"type":"assistant.message", "data":{"content":"feat: offline response"}}})
                }
                method => panic!("unexpected runtime method {method}"),
            };
            let reply = json!({"jsonrpc":"2.0", "id":request["id"], "result":result}).to_string();
            output.write_all(format!("Content-Length: {}\r\n\r\n{}", reply.len(), reply).as_bytes()).await.unwrap();
            output.flush().await.unwrap();
        }
    });
}

#[tokio::test]
async fn copilot_turns_and_cancellation_use_the_real_bridge_offline() {
    let f = Fixture::new();
    fs::write(f.repo.join("file.txt"), "initial\n").unwrap();
    f.repo_git(&["add", "--", "file.txt"]);
    f.repo_git(&["commit", "-qm", "initial"]);
    fs::write(f.repo.join("file.txt"), "changed\n").unwrap();
    f.repo_git(&["add", "--", "file.txt"]);
    fs::write(f.repo.join("file.txt"), "unstaged private text\n").unwrap();
    fs::write(f.repo.join("other.txt"), "excluded staged text\n").unwrap();
    f.repo_git(&["add", "--", "other.txt"]);
    let before = fs::read(f.repo.join(".git/index")).unwrap();
    let status = f.repo_git(&["status", "--porcelain=v2"]);
    let root = f.root.join("runtime");
    let arch = if std::env::consts::ARCH == "x86_64" {
        "x64"
    } else {
        std::env::consts::ARCH
    };
    let bin = root
        .join("prebuilds")
        .join(format!("{}-{arch}", std::env::consts::OS));
    fs::create_dir_all(&bin).unwrap();
    let executable = std::env::current_exe()
        .unwrap()
        .to_string_lossy()
        .replace('\'', "'\\''");
    let wrapper = bin.join("copilot-runtime");
    fs::write(&wrapper, format!("#!/bin/sh\nexec 3>&1\nexec '{executable}' --exact fake_copilot_runtime_child --nocapture >/dev/null\n")).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let mut server = Command::new(env!("CARGO_BIN_EXE_desktop-rs"))
        .arg("-C")
        .arg(&f.repo)
        .arg("--config")
        .arg(f.root.join("registry.json"))
        .args(["serve", "--port", "0"])
        .env("HOME", &f.root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", f.root.join("gitconfig"))
        .env("COPILOT_RUNTIME_ROOT", &root)
        .env("DESKTOP_COPILOT_NATIVE_TURN", "1")
        .env("DESKTOP_FAKE_COPILOT", "1")
        .env(
            "DESKTOP_COPILOT_MARKER",
            f.root.join("copilot-turn-started"),
        )
        .env("DESKTOP_COPILOT_PROMPT", f.root.join("copilot-prompt"))
        .env("COPILOT_TOKEN", "synthetic-copilot-secret")
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(server.stderr.take().unwrap()).lines();
    let line = timeout(Duration::from_secs(10), stderr.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let (origin, token) = line
        .strip_prefix("Open ")
        .unwrap()
        .split_once("/#token=")
        .unwrap();
    let mut client = Client::connect(origin, token).await;
    client.send(json!({"id":1,"method":"copilot_probe"})).await;
    assert_eq!(client.receive().await["result"]["available"], true);
    client
        .send(json!({"id":2,"method":"copilot_probe", "runtime_root":f.root.join("missing")}))
        .await;
    assert_eq!(client.receive().await["result"]["available"], false);
    client
        .send(json!({"id":3,"method":"copilot_create_session", "model":"auto"}))
        .await;
    let created = client.receive().await;
    let session = created["result"]["id"].as_str().unwrap();
    client.send(json!({"id":4,"method":"copilot_send_message", "session_id":session, "message":"generate commit message"})).await;
    assert_eq!(
        client.receive().await["result"]["content"],
        "feat: offline response"
    );
    client
        .send(json!({"id":5,"method":"copilot_generate_commit_message", "paths":["file.txt"]}))
        .await;
    assert_eq!(
        client.receive().await["result"]["summary"],
        "feat: offline response"
    );
    let prompt = fs::read_to_string(f.root.join("copilot-prompt")).unwrap();
    assert!(prompt.contains("+changed"));
    assert!(!prompt.contains("unstaged private text"));
    assert!(!prompt.contains("excluded staged text"));
    client
        .send(json!({"id":9,"method":"copilot_generate_commit_message", "paths":["../outside"]}))
        .await;
    assert!(client.receive().await["error"].is_string());
    client
        .send(json!({"id":10,"method":"copilot_generate_commit_message", "paths":["missing.txt"]}))
        .await;
    assert!(
        client.receive().await["error"]
            .as_str()
            .unwrap()
            .contains("no staged changes")
    );
    client
        .send(
            json!({"id":6,"method":"copilot_send_message", "session_id":session, "message":"hang"}),
        )
        .await;
    wait_for("Copilot turn did not start", || {
        f.root.join("copilot-turn-started").exists()
    })
    .await;
    client
        .send(json!({"id":7,"method":"cancel", "request_id":6}))
        .await;
    let replies = client.replies(&[6, 7]).await;
    assert_eq!(replies[&6]["error_code"], "cancelled");
    assert_eq!(replies[&7]["result"]["status"], "requested");
    assert!(!format!("{created}{replies:?}").contains("synthetic-copilot-secret"));
    client
        .send(json!({"id":8,"method":"get_preferences"}))
        .await;
    assert!(client.receive().await["result"].is_object());
    assert_eq!(fs::read(f.repo.join(".git/index")).unwrap(), before);
    assert_eq!(f.repo_git(&["status", "--porcelain=v2"]), status);
    server.kill().await.unwrap();
    server.wait().await.unwrap();
}

#[tokio::test]
async fn cancellation_preserves_git_and_cleans_processes_across_sessions() {
    let f = Fixture::new();
    fs::write(f.repo.join("file.txt"), "before\n").unwrap();
    f.repo_git(&["add", "--", "file.txt"]);
    f.repo_git(&["commit", "-qm", "initial"]);
    let original = f.repo_git(&["rev-parse", "HEAD"]);
    let root_id = f.cli(&["-C", f.repo.to_str().unwrap(), "snapshot"])["root_id"]
        .as_str()
        .unwrap()
        .to_owned();
    fs::write(f.repo.join("file.txt"), "staged\n").unwrap();
    f.repo_git(&["add", "--", "file.txt"]);
    let hook = f.repo.join(".git/hooks/pre-commit");
    executable(&hook);
    let tools = f.root.join("bin");
    fs::create_dir_all(&tools).unwrap();
    executable(&tools.join("gh"));
    let path = std::env::join_paths(std::iter::once(tools).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    let mut server = Command::new(env!("CARGO_BIN_EXE_desktop-rs"))
        .arg("-C")
        .arg(&f.repo)
        .arg("--config")
        .arg(f.root.join("registry.json"))
        .args(["serve", "--port", "0"])
        .env("HOME", &f.root)
        .env("PATH", path)
        .env("CANCELLATION_LOG", &f.log)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", f.root.join("gitconfig"))
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(server.stderr.take().unwrap()).lines();
    let line = timeout(Duration::from_secs(10), stderr.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let url = line.strip_prefix("Open ").expect("missing session URL");
    let (origin, token) = url.split_once("/#token=").expect("invalid session URL");
    let mut a = Client::connect(origin, token).await;
    let mut b = Client::connect(origin, token).await;
    a.send(json!({"id":1,"method":"commit","summary":"must not commit"}))
        .await;
    wait_for("commit hook did not start", || f.records().len() == 1).await;
    let hook_pids = f.records()[0].clone();
    b.send(json!({"id":1,"method":"snapshot"})).await;
    b.send(json!({"id":2,"method":"cancel","request_id":1}))
        .await;
    let replies = b.replies(&[1, 2]).await;
    assert_eq!(replies[&1]["error_code"], "cancelled");
    assert!(hook_pids.iter().all(|pid| alive(*pid)));
    a.send(json!({"id":2,"method":"create_branch","name":"never-created"}))
        .await;
    a.send(json!({"id":3,"method":"get_preferences"})).await;
    a.send(json!({"id":100,"method":"cancel","request_id":2}))
        .await;
    let replies = a.replies(&[2, 100]).await;
    assert_eq!(replies[&2]["error_code"], "cancelled");
    assert_eq!(replies[&100]["result"]["status"], "queued");
    a.send(json!({"id":101,"method":"cancel","request_id":1,"unexpected":true}))
        .await;
    assert!(a.receive().await.get("error").is_some());
    assert!(alive(hook_pids[0]));
    a.send(json!({"id":102,"method":"cancel","request_id":1}))
        .await;
    let replies = a.replies(&[1, 3, 102]).await;
    assert_eq!(replies[&1]["error_code"], "cancelled");
    assert_eq!(replies[&102]["result"]["status"], "requested");
    assert!(replies[&3]["result"].get("theme").is_some());
    wait_for("hook descendants survived", || {
        hook_pids.iter().all(|pid| !alive(*pid))
    })
    .await;
    assert_eq!(f.repo_git(&["rev-parse", "HEAD"]), original);
    assert!(!f.repo.join(".git/index.lock").exists());
    assert!(!f.repo_git(&["branch", "--list"]).contains("never-created"));
    assert_eq!(f.repo_git(&["show", ":file.txt"]), "staged\n");
    fs::remove_file(hook).unwrap();

    f.cli(&["preferences", "set", r#"{"theme":"light"}"#]);
    let preference = f.root.join("registry.preferences.json");
    let original_preferences = fs::read(&preference).unwrap();
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(preference.with_extension("lock"))
        .unwrap();
    lock.lock().unwrap();
    b.send(json!({"id":10,"method":"update_preferences","preferences":{"theme":"dark"}}))
        .await;
    sleep(Duration::from_millis(100)).await;
    b.send(json!({"id":11,"method":"cancel","request_id":10}))
        .await;
    let replies = b.replies(&[10, 11]).await;
    assert_eq!(replies[&10]["error_code"], "cancelled");
    assert_eq!(fs::read(&preference).unwrap(), original_preferences);
    drop(lock);

    a.gh(200).await;
    wait_for("gh did not start", || f.records().len() == 2).await;
    let gh_pids = f.records()[1].clone();
    b.send(json!({"id":201,"method":"cancel","request_id":200}))
        .await;
    assert_eq!(b.receive().await["result"]["status"], "not_found");
    assert!(gh_pids.iter().all(|pid| alive(*pid)));
    a.send(json!({"id":300,"method":"create_branch","name":"wrong-root","repository_id":root_id}))
        .await;
    for id in 301..331 {
        a.send(json!({"id":id,"method":"get_preferences"})).await;
    }
    a.send(json!({"id":400,"method":"get_preferences"})).await;
    assert_eq!(a.receive().await["error_code"], "queue_full");
    let other = f.root.join("other");
    f.git(&["init", "-q", "-b", "main", other.to_str().unwrap()]);
    f.git(&[
        "-C",
        other.to_str().unwrap(),
        "-c",
        "user.name=Cancellation Test",
        "-c",
        "user.email=cancel@example.invalid",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "--allow-empty",
        "-qm",
        "other",
    ]);
    b.send(json!({"id":202,"method":"open_repository","path":other}))
        .await;
    assert_ne!(b.receive().await["result"]["root_id"], root_id);
    a.send(json!({"id":401,"method":"cancel","request_id":200}))
        .await;
    let mut expected = vec![200, 401];
    expected.extend(300..331);
    let replies = a.replies(&expected).await;
    assert!(
        replies[&300]["error"]
            .as_str()
            .unwrap()
            .contains("selection changed")
    );
    assert!(
        !f.repo_git(&["branch", "--list", "wrong-root"])
            .contains("wrong-root")
    );
    assert!(
        f.git(&[
            "-C",
            other.to_str().unwrap(),
            "branch",
            "--list",
            "wrong-root"
        ])
        .is_empty()
    );
    wait_for("cancel left gh descendants", || {
        gh_pids.iter().all(|pid| !alive(*pid))
    })
    .await;

    a.gh(210).await;
    wait_for("disconnect fixture did not start", || {
        f.records().len() == 3
    })
    .await;
    let disconnected = f.records()[2].clone();
    a.0.close(None).await.unwrap();
    wait_for("disconnect left descendants", || {
        disconnected.iter().all(|pid| !alive(*pid))
    })
    .await;
    let mut c = Client::connect(origin, token).await;
    c.gh(500).await;
    wait_for("duplicate fixture did not start", || f.records().len() == 4).await;
    let duplicate = f.records()[3].clone();
    c.send(json!({"id":500,"method":"get_preferences"})).await;
    let reply = c.receive().await;
    assert!(reply["id"].is_null() && reply["error"].as_str().unwrap().contains("duplicate"));
    wait_for("duplicate ID left descendants", || {
        duplicate.iter().all(|pid| !alive(*pid))
    })
    .await;
    b.gh(600).await;
    wait_for("shutdown fixture did not start", || f.records().len() == 5).await;
    let shutdown = f.records()[4].clone();
    assert!(signal(server.id().unwrap(), "INT"));
    assert!(
        timeout(Duration::from_secs(10), server.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    wait_for("shutdown left descendants", || {
        shutdown.iter().all(|pid| !alive(*pid))
    })
    .await;
}

fn editor_preference(f: &Fixture, editor: Value) {
    f.cli(&[
        "preferences",
        "set",
        &json!({"external_editor":editor}).to_string(),
    ]);
}
fn editor_command(f: &Fixture) -> std::process::Command {
    let mut cmd = f.command(env!("CARGO_BIN_EXE_desktop-rs"));
    cmd.arg("-C")
        .arg(&f.repo)
        .arg("--config")
        .arg(f.root.join("registry.json"));
    cmd
}
fn editor_script(path: &Path, source: &str) {
    fs::write(path, source).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[test]
fn editor_cli_uses_literal_raw_arguments_and_preserves_terminal_status() {
    let f = Fixture::new();
    fs::write(f.repo.join("file"), b"base\n").unwrap();
    let old_preferences = f.root.join("registry.preferences.json");
    let old = b"{\"version\":1,\"theme\":\"system\",\"poll_interval_ms\":2000}";
    fs::write(&old_preferences, old).unwrap();
    assert_eq!(
        f.cli(&["preferences"])["external_editor"],
        json!({"kind":"automatic"})
    );
    assert_eq!(fs::read(&old_preferences).unwrap(), old);
    for bad in [
        json!({"kind":"command","program":""}),
        json!({"kind":"command","program":"editor","args":vec!["x";65]}),
        json!({"kind":"unknown"}),
    ] {
        let result = editor_command(&f)
            .args([
                "preferences",
                "set",
                &json!({"external_editor":bad}).to_string(),
            ])
            .output()
            .unwrap();
        assert!(!result.status.success());
    }
    let script = f.root.join("record editor.sh");
    let marker = f.root.join("editor-args");
    editor_script(
        &script,
        "#!/bin/sh\nout=$1; shift\nprintf '%s\\0' \"$@\" > \"$out\"\npwd > \"$out.cwd\"\nif [ -t 0 ]; then printf yes > \"$out.tty\"; else printf no > \"$out.tty\"; fi\n",
    );
    let command = json!({"kind":"command","program":script,"args":[marker,"{path}","{repo}","$(touch must-not-exist)"],"terminal":false});
    editor_preference(&f, command.clone());
    let info = f.cli(&["editor", "--list"]);
    assert_eq!(info["configuration"], command);
    assert!(info["editors"].is_array());
    let raw = std::ffi::OsString::from_vec(b"raw-\xff \" $(touch must-not-exist)\n.txt".to_vec());
    let raw_path = f.repo.join(&raw);
    fs::write(&raw_path, b"raw file\n").unwrap();
    let output = editor_command(&f).arg("editor").arg(&raw).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut expected = Vec::new();
    for value in [
        raw_path.as_os_str().as_bytes(),
        f.repo.as_os_str().as_bytes(),
        b"$(touch must-not-exist)".as_slice(),
    ] {
        expected.extend(value);
        expected.push(0);
    }
    assert_eq!(fs::read(&marker).unwrap(), expected);
    assert_eq!(
        fs::read_to_string(f.root.join("editor-args.cwd"))
            .unwrap()
            .trim_end(),
        f.repo.to_str().unwrap()
    );
    assert!(!f.repo.join("must-not-exist").exists());
    editor_preference(
        &f,
        json!({"kind":"command","program":script,"args":[marker,"prefix={repo}"],"terminal":false}),
    );
    assert!(
        editor_command(&f)
            .args(["editor", "file"])
            .status()
            .unwrap()
            .success()
    );
    let mut expected = format!("prefix={}", f.repo.display()).into_bytes();
    expected.push(0);
    expected.extend(f.repo.join("file").as_os_str().as_bytes());
    expected.push(0);
    assert_eq!(fs::read(&marker).unwrap(), expected);
    let alias = f.root.join("shell-editor-alias");
    symlink("/bin/sh", &alias).unwrap();
    let native = f
        .command(alias.to_str().unwrap())
        .args(["-c", "tr '\\000' '\\n' </proc/$$/cmdline | head -n 1"])
        .output()
        .unwrap();
    assert!(native.status.success());
    editor_preference(
        &f,
        json!({"kind":"command","program":alias,"args":["-c","tr '\\000' '\\n' </proc/$$/cmdline | head -n 1"],"terminal":false}),
    );
    let via_editor = editor_command(&f)
        .args(["editor", "file"])
        .output()
        .unwrap();
    assert!(via_editor.status.success());
    assert_eq!(via_editor.stdout, native.stdout);
    let outside = f.root.join("outside");
    fs::write(&outside, "private").unwrap();
    symlink(outside, f.repo.join("outside-link")).unwrap();
    symlink(f.repo.join(".git/config"), f.repo.join("metadata-link")).unwrap();
    for path in [
        "../outside",
        ".git/config",
        "outside-link",
        "metadata-link",
        "missing",
    ] {
        assert!(
            !editor_command(&f)
                .args(["editor", path])
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let tty = f.root.join("tty-editor");
    editor_script(
        &tty,
        "#!/bin/sh\nread -r input\n[ -t 0 ] && exit 37\nexit 3\n",
    );
    editor_preference(
        &f,
        json!({"kind":"command","program":tty,"args":[],"terminal":true}),
    );
    let binary = shell_quote(env!("CARGO_BIN_EXE_desktop-rs"));
    let repo = shell_quote(f.repo.to_str().unwrap());
    let config = shell_quote(f.root.join("registry.json").to_str().unwrap());
    let line = format!("{binary} -C {repo} --config {config} editor file");
    let mut process = f
        .command("script")
        .args(["-q", "-e", "-c", &line, "/dev/null"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    use std::io::Write as _;
    process
        .stdin
        .take()
        .unwrap()
        .write_all(b"terminal input\n")
        .unwrap();
    assert_eq!(process.wait().unwrap().code(), Some(37));
}

#[tokio::test]
async fn editor_bridge_fences_repository_paths_and_preserves_detached_editor() {
    let f = Fixture::new();
    fs::write(f.repo.join("file"), b"base\n").unwrap();
    let outside = f.root.join("outside");
    fs::write(&outside, b"outside").unwrap();
    symlink(&outside, f.repo.join("outside-link")).unwrap();
    symlink(f.repo.join(".git/config"), f.repo.join("metadata-link")).unwrap();
    let script = f.root.join("editor.sh");
    let marker = f.root.join("editor-args");
    editor_script(
        &script,
        "#!/bin/sh\nout=$1; shift\nprintf '%s\\0' \"$@\" > \"$out\"\nif [ -t 0 ]; then printf yes > \"$out.tty\"; else printf no > \"$out.tty\"; fi\n",
    );
    editor_preference(
        &f,
        json!({"kind":"command","program":script,"args":[marker,"{path}"],"terminal":true}),
    );
    let mut server = Command::new(env!("CARGO_BIN_EXE_desktop-rs"))
        .arg("-C")
        .arg(&f.repo)
        .arg("--config")
        .arg(f.root.join("registry.json"))
        .args(["serve", "--port", "0"])
        .env("HOME", &f.root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", f.root.join("gitconfig"))
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(server.stderr.take().unwrap()).lines();
    let line = timeout(Duration::from_secs(10), stderr.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let (origin, token) = line
        .strip_prefix("Open ")
        .unwrap()
        .split_once("/#token=")
        .unwrap();
    let mut client = Client::connect(origin, token).await;
    client.send(json!({"id":1,"method":"snapshot"})).await;
    let snapshot = client.receive().await;
    let selected = snapshot["result"]["root_id"].as_str().unwrap();
    client
        .send(json!({"id":2,"method":"open_editor","path":"file","repository_id":selected}))
        .await;
    assert!(
        client.receive().await["error"]
            .as_str()
            .unwrap()
            .contains("interactive terminal")
    );
    client
        .send(json!({"id":3,"method":"open_editor","path":"file","repository_id":"obsolete"}))
        .await;
    assert!(client.receive().await["error"].is_string());
    let raw = std::ffi::OsString::from_vec(b"raw-\xff \" $(touch must-not-exist)\n.txt".to_vec());
    let raw_path = f.repo.join(&raw);
    fs::write(&raw_path, b"raw\n").unwrap();
    let editor =
        json!({"kind":"command","program":script,"args":[marker,"{path}"],"terminal":false});
    editor_preference(&f, editor.clone());
    client.send(json!({"id":4,"method":"open_editor","path":{"bytes":raw.as_bytes()},"repository_id":selected})).await;
    assert_eq!(client.receive().await["result"]["code"], 0);
    wait_for("editor argument recording", || marker.exists()).await;
    let mut expected = raw_path.as_os_str().as_bytes().to_vec();
    expected.push(0);
    assert_eq!(fs::read(&marker).unwrap(), expected);
    assert_eq!(
        fs::read_to_string(f.root.join("editor-args.tty")).unwrap(),
        "no"
    );
    client
        .send(json!({"id":5,"method":"available_editors","repository_id":"obsolete"}))
        .await;
    assert_eq!(client.receive().await["result"]["configuration"], editor);
    for (id, path) in [(6, "outside-link"), (7, "metadata-link"), (8, "../outside")] {
        client
            .send(json!({"id":id,"method":"open_editor","path":path,"repository_id":selected}))
            .await;
        assert!(client.receive().await["error"].is_string());
    }
    let fail = f.root.join("fail-editor");
    editor_script(&fail, "#!/bin/sh\nexit 17\n");
    editor_preference(
        &f,
        json!({"kind":"command","program":fail,"args":[],"terminal":false}),
    );
    client
        .send(json!({"id":9,"method":"open_editor","path":"file"}))
        .await;
    assert!(
        client.receive().await["error"]
            .as_str()
            .unwrap()
            .contains("17")
    );
    let persistent = f.root.join("persistent-editor");
    let pid_file = f.root.join("editor-pid");
    let stop = f.root.join("stop-editor");
    editor_script(
        &persistent,
        "#!/bin/sh\nprintf '%s' \"$$\" > \"$1\"\nwhile [ ! -e \"$2\" ]; do sleep 0.05; done\n",
    );
    editor_preference(
        &f,
        json!({"kind":"command","program":persistent,"args":[pid_file,stop],"terminal":false}),
    );
    client
        .send(json!({"id":10,"method":"open_editor","path":"file"}))
        .await;
    let reply = timeout(Duration::from_secs(3), client.receive())
        .await
        .unwrap();
    assert_eq!(reply["result"]["code"], 0);
    wait_for("persistent editor start", || pid_file.exists()).await;
    let pid: u32 = fs::read_to_string(&pid_file).unwrap().parse().unwrap();
    let pgid = f
        .command("ps")
        .args(["-o", "pgid=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(pgid.stdout).unwrap().trim(),
        pid.to_string()
    );
    client.send(json!({"id":11,"method":"snapshot"})).await;
    assert_eq!(client.receive().await["result"]["root_id"], selected);
    client.0.close(None).await.unwrap();
    assert!(signal(server.id().unwrap(), "INT"));
    timeout(Duration::from_secs(10), server.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(alive(pid));
    fs::write(stop, b"").unwrap();
    wait_for("persistent editor stopped", || !alive(pid)).await;
}
