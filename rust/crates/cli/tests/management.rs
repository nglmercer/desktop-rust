//! Repository management through the CLI and authenticated loopback bridge.
#![cfg(unix)]

use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::TcpStream,
    time::timeout,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

struct Fixture {
    root: PathBuf,
    repo: PathBuf,
    registry: PathBuf,
    gitconfig: PathBuf,
    secret: Option<String>,
}

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("desktop-management-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let repo = root.join("repo");
        let registry = root.join("config/repositories.json");
        let gitconfig = root.join("global.gitconfig");
        fs::write(&gitconfig, "[protocol \"file\"]\n allow = always\n[user]\n name = Fixture\n email = fixture@example.invalid\n[commit]\n gpgsign = false\n").unwrap();
        let f = Self {
            root,
            repo,
            registry,
            gitconfig,
            secret: None,
        };
        f.git(&["init", "-q", "-b", "main", f.repo.to_str().unwrap()]);
        f
    }

    fn setup(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .env("HOME", &self.root)
            .env("GIT_CONFIG_GLOBAL", &self.gitconfig)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS");
        if let Some(secret) = &self.secret {
            command.env("GH_TOKEN", secret);
        }
        command
    }

    fn git(&self, args: &[&str]) -> Vec<u8> {
        let output = self.setup("git").args(args).output().unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    fn git_in(&self, dir: &Path, args: &[&str]) -> Vec<u8> {
        let mut all = vec!["-C", dir.to_str().unwrap()];
        all.extend_from_slice(args);
        self.git(&all)
    }

    fn repo_git(&self, args: &[&str]) -> Vec<u8> {
        self.git_in(&self.repo, args)
    }

    fn cli_raw<I, S>(&self, args: I, fail: bool) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self
            .setup(env!("CARGO_BIN_EXE_desktop-rs"))
            .arg("--config")
            .arg(&self.registry)
            .arg("-C")
            .arg(&self.repo)
            .args(args)
            .output()
            .unwrap();
        if let Some(secret) = &self.secret {
            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(!combined.contains(secret));
            assert!(!combined.contains(&STANDARD.encode(secret)));
        }
        assert_eq!(
            output.status.success(),
            !fail,
            "CLI: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn cli(&self, args: &[&str], fail: bool) -> Value {
        let output = self.cli_raw(args, fail);
        if fail {
            Value::Null
        } else {
            serde_json::from_slice(&output.stdout).unwrap()
        }
    }

    fn rpc(&self, method: &str, mut payload: Value, fail: bool) -> Value {
        payload
            .as_object_mut()
            .unwrap()
            .insert("method".into(), method.into());
        self.cli(&["repo", &payload.to_string()], fail)
    }
}

#[test]
fn repository_git_identity_reads_preserve_git_scope_and_values() {
    let fixture = Fixture::new();
    let read = |key: &str, only_local: bool| {
        fixture.rpc(
            "git_identity_value",
            json!({"key":key,"only_local":only_local}),
            false,
        )
    };
    assert!(read("user.name", true).is_null());
    assert_eq!(read("user.name", false), "Fixture");
    fixture.repo_git(&["config", "--add", "user.name", "First"]);
    fixture.repo_git(&["config", "--add", "user.name", "Last"]);
    assert_eq!(read("user.name", true), "Last");
    assert_eq!(read("user.name", false), "Last");
    fixture.repo_git(&["config", "--replace-all", "user.email", ""]);
    assert_eq!(read("user.email", true), "");
    fixture.rpc(
        "git_identity_value",
        json!({"key":"credential.helper","only_local":true}),
        true,
    );
}

#[test]
fn git_setting_write_failure_is_not_confused_with_an_absent_unset() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    let real_git = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    assert!(real_git.status.success());
    let real_git = String::from_utf8(real_git.stdout).unwrap();
    let bin = fixture.root.join("git-wrapper");
    fs::create_dir(&bin).unwrap();
    let wrapper = bin.join("git");
    fs::write(
        &wrapper,
        "#!/bin/sh\ncase \" $* \" in *' --replace-all '*) echo 'synthetic write failure' >&2; exit 5;; esac\nexec \"$TEST_REAL_GIT\" \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let before = fs::read(&fixture.gitconfig).unwrap();
    let output = fixture
        .setup(env!("CARGO_BIN_EXE_desktop-rs"))
        .env("TEST_REAL_GIT", real_git.trim())
        .env(
            "PATH",
            std::env::join_paths(
                std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())),
            )
            .unwrap(),
        )
        .arg("--config")
        .arg(&fixture.registry)
        .args(["-C", fixture.repo.to_str().unwrap(), "repo"])
        .arg(json!({"method":"set_git_setting","scope":"global","key":"user.name","value":"New Name"}).to_string())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("synthetic write failure"));
    assert_eq!(fs::read(&fixture.gitconfig).unwrap(), before);
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
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
        assert_eq!(client.receive(false).await["ready"], true);
        client
    }
    async fn send(&mut self, value: Value) {
        self.0.send(Message::text(value.to_string())).await.unwrap();
    }
    async fn receive(&mut self, include_events: bool) -> Value {
        loop {
            let message = timeout(Duration::from_secs(15), self.0.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            if let Message::Text(text) = message {
                let value: Value = serde_json::from_str(&text).unwrap();
                if include_events || value.get("event").is_none() {
                    return value;
                }
            }
        }
    }
}

#[test]
fn old_preferences_are_read_without_rewriting_and_can_set_checkout_strategy() {
    let f = Fixture::new();
    let preferences = f.registry.with_extension("preferences.json");
    fs::create_dir_all(preferences.parent().unwrap()).unwrap();
    let before = br#"{"version":1,"theme":"system","poll_interval_ms":2000}"#;
    fs::write(&preferences, before).unwrap();
    assert_eq!(
        f.cli(&["preferences"], false)["uncommitted_changes"],
        "AskForConfirmation"
    );
    assert_eq!(fs::read(&preferences).unwrap(), before);
    f.cli(
        &[
            "preferences",
            "set",
            r#"{"uncommitted_changes":"MoveToNewBranch"}"#,
        ],
        false,
    );
    let saved: Value = serde_json::from_slice(&fs::read(preferences).unwrap()).unwrap();
    assert_eq!(saved["uncommitted_changes"], "MoveToNewBranch");
}

#[tokio::test]
async fn public_registry_views_hide_and_preserve_publication_ledger() {
    const MARKER: &str = "8765432109876543";
    let original_home = std::env::var_os("HOME");
    let mut f = Fixture::new();
    f.secret = Some(MARKER.to_owned());
    fs::write(f.repo.join("file"), "base\n").unwrap();
    f.repo_git(&["add", "--", "file"]);
    f.repo_git(&["commit", "-qm", "initial"]);
    f.cli(&["repositories", "add", f.repo.to_str().unwrap()], false);
    let mut registry: Value = serde_json::from_slice(&fs::read(&f.registry).unwrap()).unwrap();
    let record = registry["records"][0]["id"].as_str().unwrap();
    let ledger = json!([{
        "id": uuid::Uuid::new_v4().to_string(),
        "repository_id": "a".repeat(64),
        "common_id": "b".repeat(64),
        "record_id": record,
        "phase": "created",
        "destination_hash": "c".repeat(64),
        "remote_id": 8765432109876543_u64,
        "acknowledged": false
    }]);
    registry["publication_intents"] = ledger.clone();
    fs::write(&f.registry, serde_json::to_vec(&registry).unwrap()).unwrap();
    let hidden = |value: &Value| {
        let encoded = value.to_string();
        assert!(!encoded.contains("publication_intents"));
        assert!(!encoded.contains(MARKER));
    };
    let preserved = || {
        let data: Value = serde_json::from_slice(&fs::read(&f.registry).unwrap()).unwrap();
        assert_eq!(data["publication_intents"], ledger);
    };
    hidden(&f.cli(&["repositories", "list"], false));
    hidden(&f.cli(&["repositories", "select", f.repo.to_str().unwrap()], false));
    let other = f.root.join("other");
    f.git(&["init", "-q", other.to_str().unwrap()]);
    hidden(&f.cli(&["repositories", "add", other.to_str().unwrap()], false));
    hidden(&f.cli(&["repositories", "forget", other.to_str().unwrap()], false));
    preserved();
    let created = f.root.join("initialized");
    hidden(&f.cli(&["init", created.to_str().unwrap()], false));
    preserved();
    let cloned = f.root.join("local-clone");
    hidden(&f.cli(
        &["clone", f.repo.to_str().unwrap(), cloned.to_str().unwrap()],
        false,
    ));
    preserved();
    let sibling = f.root.join("occupied-worktree");
    f.repo_git(&[
        "worktree",
        "add",
        "-qb",
        "occupied",
        sibling.to_str().unwrap(),
    ]);
    hidden(&f.cli(&["checkout", "occupied", "--open-worktree"], false));
    preserved();

    let mut server = tokio::process::Command::new(env!("CARGO_BIN_EXE_desktop-rs"))
        .arg("--config")
        .arg(&f.registry)
        .args(["serve", "--empty", "--port", "0"])
        .env("HOME", &f.root)
        .env("GIT_CONFIG_GLOBAL", &f.gitconfig)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GH_TOKEN", MARKER)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
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
    client
        .send(json!({"id":1,"method":"list_repositories"}))
        .await;
    let listed = client.receive(false).await;
    assert!(listed.get("result").is_some(), "{listed}");
    hidden(&listed);
    client
        .send(json!({"id":2,"method":"forget_repository","path":created}))
        .await;
    let forgotten = client.receive(false).await;
    assert!(forgotten.get("result").is_some(), "{forgotten}");
    hidden(&forgotten);
    preserved();
    assert_eq!(std::env::var_os("HOME"), original_home);
    client.0.close(None).await.unwrap();
    assert!(
        Command::new("kill")
            .args(["-INT", &server.id().unwrap().to_string()])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        timeout(Duration::from_secs(10), server.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}

#[tokio::test]
async fn settings_worktrees_submodules_byte_paths_and_bridge_events() {
    let original_home = std::env::var_os("HOME");
    let f = Fixture::new();
    let preferences = f.registry.with_extension("preferences.json");
    assert_eq!(
        f.cli(&["preferences"], false),
        json!({"version":1,"theme":"system","poll_interval_ms":2000,"external_editor":{"kind":"automatic"},"uncommitted_changes":"AskForConfirmation"})
    );
    assert!(!preferences.exists());
    assert_eq!(
        f.cli(&["preferences", "set", r#"{"theme":"dark"}"#], false)["theme"],
        "dark"
    );
    assert_eq!(
        f.cli(
            &["preferences", "set", r#"{"poll_interval_ms":500}"#],
            false
        )["theme"],
        "dark"
    );
    let before = fs::read(&preferences).unwrap();
    f.cli(&["preferences", "set", r#"{"poll_interval_ms":1}"#], true);
    assert_eq!(fs::read(&preferences).unwrap(), before);
    std::thread::scope(|scope| {
        let a = scope.spawn(|| f.cli(&["preferences", "set", r#"{"theme":"light"}"#], false));
        let b = scope.spawn(|| {
            f.cli(
                &["preferences", "set", r#"{"poll_interval_ms":1000}"#],
                false,
            )
        });
        a.join().unwrap();
        b.join().unwrap();
    });
    let prefs = f.cli(&["preferences"], false);
    assert_eq!(prefs["theme"], "light");
    assert_eq!(prefs["poll_interval_ms"], 1000);
    fs::write(&preferences, "{broken").unwrap();
    f.cli(&["preferences", "set", r#"{"theme":"system"}"#], true);
    assert_eq!(fs::read_to_string(&preferences).unwrap(), "{broken");
    fs::write(&preferences, before).unwrap();

    f.repo_git(&["config", "credential.helper", "fixture-secret"]);
    let values = f.cli(&["config"], false)["values"].clone();
    assert!(values.get("credential.helper").is_none() && values["user.name"].is_null());
    assert_eq!(
        f.cli(&["config", "--global"], false)["values"]["user.name"],
        "Fixture"
    );
    assert_eq!(
        f.cli(&["config", "set", "user.name", "Local Author"], false)["values"]["user.name"],
        "Local Author"
    );
    assert_eq!(
        f.cli(&["config", "--global"], false)["values"]["user.name"],
        "Fixture"
    );
    f.rpc(
        "set_git_setting",
        json!({"key":"credential.helper","value":"bad"}),
        true,
    );
    f.rpc(
        "set_git_setting",
        json!({"key":"core.autocrlf","value":"bad"}),
        true,
    );
    f.cli(
        &["config", "set", "init.defaultBranch", "--", "--help"],
        true,
    );
    for _ in 0..2 {
        assert!(f.cli(&["config", "unset", "user.name"], false)["values"]["user.name"].is_null());
    }
    assert!(
        fs::read_to_string(&f.gitconfig)
            .unwrap()
            .contains("Fixture")
    );
    assert!(
        String::from_utf8(f.repo_git(&["config", "credential.helper"]))
            .unwrap()
            .contains("fixture-secret")
    );
    f.cli(
        &["config", "set", "init.defaultBranch", "trunk", "--global"],
        false,
    );
    let created = f.root.join("configured-default");
    f.cli(&["init", created.to_str().unwrap()], false);
    assert_eq!(
        f.git_in(&created, &["symbolic-ref", "--short", "HEAD"]),
        b"trunk\n"
    );

    fs::write(f.repo.join("file"), "base\n").unwrap();
    f.repo_git(&["add", "--all"]);
    f.repo_git(&["commit", "-qm", "base"]);
    f.repo_git(&["branch", "existing"]);
    let attached = f.root.join("attached");
    f.cli(
        &["worktree", "add", attached.to_str().unwrap(), "existing"],
        false,
    );
    assert_eq!(
        f.git_in(&attached, &["symbolic-ref", "--short", "HEAD"]),
        b"existing\n"
    );
    f.cli(&["worktree", "remove", attached.to_str().unwrap()], false);
    let linked = f.root.join("linked\nwith space");
    f.rpc(
        "worktree_add",
        json!({"path":linked,"branch":"topic","new_branch":true}),
        false,
    );
    let listed = f.rpc("worktrees", json!({}), false);
    assert_eq!(listed.as_array().unwrap().len(), 2);
    assert_eq!(listed[0]["main"], true);
    let row = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["display_path"] == linked.to_str().unwrap())
        .unwrap();
    assert_eq!(row["path"]["bytes"], json!(linked.as_os_str().as_bytes()));
    assert_eq!(row["branch"], "refs/heads/topic");
    f.rpc(
        "worktree_lock",
        json!({"path":row["path"],"locked":true,"reason":"kept for work"}),
        false,
    );
    let worktrees = f.rpc("worktrees", json!({}), false);
    assert_eq!(
        worktrees
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["main"] == false)
            .unwrap()["locked"],
        "kept for work"
    );
    f.rpc("worktree_remove", json!({"path":linked}), true);
    assert!(linked.exists());
    f.rpc(
        "worktree_lock",
        json!({"path":linked,"locked":false}),
        false,
    );
    fs::write(linked.join("untracked"), "do not lose").unwrap();
    f.rpc("worktree_remove", json!({"path":linked}), true);
    assert_eq!(
        fs::read_to_string(linked.join("untracked")).unwrap(),
        "do not lose"
    );
    let moved = f.root.join("moved");
    f.rpc("worktree_move", json!({"from":linked,"to":moved}), false);
    assert_eq!(
        fs::read_to_string(moved.join("untracked")).unwrap(),
        "do not lose"
    );
    assert!(!linked.exists());
    f.rpc("worktree_remove", json!({"path":f.repo,"force":true}), true);
    assert!(f.repo.exists());
    fs::remove_file(moved.join("untracked")).unwrap();
    f.rpc("worktree_remove", json!({"path":moved}), false);
    assert!(!moved.exists());
    f.rpc(
        "worktree_remove",
        json!({"path":f.root.join("not-registered"),"force":true}),
        true,
    );
    assert_eq!(f.cli(&["worktree"], false).as_array().unwrap().len(), 1);

    let source = f.root.join("source");
    f.git(&["init", "-q", source.to_str().unwrap()]);
    fs::write(source.join("source-file"), "source").unwrap();
    f.git_in(&source, &["add", "--all"]);
    f.git_in(&source, &["commit", "-qm", "source"]);
    f.rpc(
        "submodule_add",
        json!({"url":source,"path":"modules/with space","name":"fixture"}),
        false,
    );
    f.rpc(
        "submodule_deinit",
        json!({"paths":["modules/with space"]}),
        true,
    );
    f.repo_git(&["commit", "-qm", "Register submodule"]);
    let modules = f.cli(&["submodule"], false);
    assert_eq!(modules.as_array().unwrap().len(), 1);
    assert_eq!(modules[0]["initialized"], true);
    assert_eq!(modules[0]["dirty"], false);
    assert_eq!(modules[0]["changed"], false);
    let nested = f.repo.join("modules/with space");
    fs::write(nested.join("untracked"), "dirty").unwrap();
    assert_eq!(f.rpc("submodules", json!({}), false)[0]["dirty"], true);
    f.rpc(
        "submodule_deinit",
        json!({"paths":["modules/with space"]}),
        true,
    );
    assert!(nested.join("untracked").exists());
    fs::remove_file(nested.join("untracked")).unwrap();
    f.rpc(
        "submodule_deinit",
        json!({"paths":["modules/with space"]}),
        false,
    );
    assert_eq!(
        f.rpc("submodules", json!({}), false)[0]["initialized"],
        false
    );
    f.rpc(
        "submodule_update",
        json!({"paths":["modules/with space"]}),
        false,
    );
    assert_eq!(
        fs::read_to_string(nested.join("source-file")).unwrap(),
        "source"
    );
    f.rpc(
        "submodule_sync",
        json!({"paths":["modules/with space"]}),
        false,
    );
    assert_eq!(f.cli(&["submodule"], false)[0]["initialized"], true);
    f.rpc("submodule_deinit", json!({"paths":[]}), true);
    f.rpc(
        "submodule_add",
        json!({"url":"ext::bad","path":"bad"}),
        true,
    );

    let oid = |args: &[&str]| {
        String::from_utf8(f.git_in(&nested, args))
            .unwrap()
            .trim()
            .to_owned()
    };
    let base = oid(&["rev-parse", "HEAD"]);
    fs::write(nested.join("source-file"), "ours").unwrap();
    f.git_in(&nested, &["add", "--all"]);
    f.git_in(&nested, &["commit", "-qm", "ours"]);
    let ours = oid(&["rev-parse", "HEAD"]);
    fs::write(nested.join("source-file"), "theirs").unwrap();
    f.git_in(&nested, &["add", "--all"]);
    f.git_in(&nested, &["commit", "-qm", "theirs"]);
    let theirs = oid(&["rev-parse", "HEAD"]);
    f.git_in(&nested, &["checkout", "-q", &ours]);
    fs::write(nested.join("local-only"), "preserved").unwrap();
    let mut input = format!("0 {}\tmodules/with space\0", "0".repeat(40)).into_bytes();
    for (stage, hash) in [(1, &base), (2, &ours), (3, &theirs)] {
        input.extend_from_slice(format!("160000 {hash} {stage}\tmodules/with space\0").as_bytes());
    }
    let mut child = f
        .setup("git")
        .arg("-C")
        .arg(&f.repo)
        .args(["update-index", "-z", "--index-info"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&input).unwrap();
    assert!(child.wait().unwrap().success());
    let conflict = f.rpc(
        "conflict_detail",
        json!({"path":"modules/with space"}),
        false,
    );
    f.rpc(
        "resolve_conflict",
        json!({"path":"modules/with space","choice":"theirs","basis":conflict["basis"]}),
        false,
    );
    assert!(
        String::from_utf8(f.repo_git(&["ls-files", "--stage", "--", "modules/with space"]))
            .unwrap()
            .starts_with(&format!("160000 {theirs} 0"))
    );
    assert!(
        f.repo_git(&["ls-files", "--unmerged", "--", "modules/with space"])
            .is_empty()
    );
    assert_eq!(oid(&["rev-parse", "HEAD"]), ours);
    assert!(nested.join("local-only").exists());

    let mut roots = Vec::new();
    let mut native = PathBuf::new();
    for suffix in [0xff, 0xfe] {
        let mut bytes = f.root.as_os_str().as_bytes().to_vec();
        bytes.extend_from_slice(b"/raw-");
        bytes.push(suffix);
        native = PathBuf::from(OsString::from_vec(bytes.clone()));
        let saved: Value = serde_json::from_slice(
            &f.cli_raw([OsStr::new("init"), native.as_os_str()], false)
                .stdout,
        )
        .unwrap();
        assert_eq!(saved["selected"]["bytes"], json!(bytes));
        let selected = f
            .setup(env!("CARGO_BIN_EXE_desktop-rs"))
            .arg("--config")
            .arg(&f.registry)
            .args(["--selected", "snapshot"])
            .output()
            .unwrap();
        assert!(
            selected.status.success(),
            "{}",
            String::from_utf8_lossy(&selected.stderr)
        );
        let snapshot: Value = serde_json::from_slice(&selected.stdout).unwrap();
        roots.push(snapshot["root_id"].as_str().unwrap().to_owned());
    }
    assert_ne!(roots[0], roots[1]);
    let mut server = tokio::process::Command::new(env!("CARGO_BIN_EXE_desktop-rs"))
        .arg("--config")
        .arg(&f.registry)
        .args(["serve", "--empty", "--port", "0"])
        .env("HOME", &f.root)
        .env("GIT_CONFIG_GLOBAL", &f.gitconfig)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
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
    let mut a = Client::connect(origin, token).await;
    let mut b = Client::connect(origin, token).await;
    a.send(json!({"id":1,"method":"git_settings","scope":"global"}))
        .await;
    assert_eq!(
        a.receive(false).await["result"]["values"]["user.name"],
        "Fixture"
    );
    a.send(json!({"id":2,"method":"update_preferences","preferences":{"theme":"light","poll_interval_ms":500}})).await;
    assert_eq!(a.receive(false).await["result"]["theme"], "light");
    let event = b.receive(true).await;
    assert_eq!(event["event"], "preferences_changed");
    assert_eq!(event["preferences"]["theme"], "light");
    a.send(
        json!({"id":3,"method":"open_repository","path":{"bytes":native.as_os_str().as_bytes()}}),
    )
    .await;
    assert_eq!(a.receive(false).await["result"]["root_id"], roots[1]);
    a.send(json!({"id":4,"method":"worktrees"})).await;
    assert_eq!(
        a.receive(false).await["result"][0]["path"]["bytes"],
        json!(native.as_os_str().as_bytes())
    );
    a.send(
        json!({"id":99,"method":"update_preferences","preferences":{"poll_interval_ms":"invalid"}}),
    )
    .await;
    let invalid = a.receive(false).await;
    assert_eq!(invalid["id"], 99);
    assert!(invalid.get("error").is_some());
    a.send(json!({"id":100,"method":"get_preferences"})).await;
    assert_eq!(a.receive(false).await["result"]["theme"], "light");
    a.0.close(None).await.unwrap();
    b.0.close(None).await.unwrap();
    assert!(
        Command::new("kill")
            .args(["-INT", &server.id().unwrap().to_string()])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        timeout(Duration::from_secs(10), server.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    assert_eq!(std::env::var_os("HOME"), original_home);
}

#[tokio::test]
async fn repository_discovery_reports_dubious_owner_without_selecting_repository() {
    let f = Fixture::new();
    let nested = f.repo.join("nested");
    fs::create_dir_all(&nested).unwrap();
    let name = format!(
        "repository'\nfatal: detected dubious ownership in repository at '{}'\nfatal: detected dubious ownership in repository at '/*'\nend",
        f.root.display()
    );
    let unusual = f.root.join(name);
    fs::create_dir_all(unusual.join("nested")).unwrap();
    let wildcard = f.root.join("*");
    fs::create_dir_all(&wildcard).unwrap();
    f.git(&["init", "-q", unusual.to_str().unwrap()]);
    let mut server = tokio::process::Command::new(env!("CARGO_BIN_EXE_desktop-rs"))
        .arg("--config")
        .arg(&f.registry)
        .args(["serve", "--empty", "--port", "0"])
        .env("HOME", &f.root)
        .env("GIT_CONFIG_GLOBAL", &f.gitconfig)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TEST_ASSUME_DIFFERENT_OWNER", "1")
        .env("GIT_TRACE", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
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
    client
        .send(json!({"id":1,"method":"repository_type","path":nested}))
        .await;
    let result = client.receive(false).await;
    assert_eq!(
        result["result"],
        json!({"kind":"unsafe","path":f.repo.canonicalize().unwrap()}),
        "{result}"
    );
    client
        .send(json!({"id":3,"method":"repository_type","path":unusual.join("nested")}))
        .await;
    let result = client.receive(false).await;
    assert_eq!(
        result["result"],
        json!({"kind":"unsafe","path":unusual.canonicalize().unwrap()}),
        "{result}"
    );
    let original_config = fs::read(&f.gitconfig).unwrap();
    for (id, path) in [
        (6, unusual.join("nested")),
        (7, wildcard),
        (8, f.root.join("missing")),
    ] {
        client
            .send(json!({"id":id,"method":"trust_repository","path":path}))
            .await;
        assert!(client.receive(false).await.get("error").is_some());
        assert_eq!(fs::read(&f.gitconfig).unwrap(), original_config);
    }
    client
        .send(json!({"id":9,"method":"trust_repository","path":unusual}))
        .await;
    assert_eq!(
        client.receive(false).await["result"],
        json!({"trusted":true})
    );
    let trusted_config = fs::read(&f.gitconfig).unwrap();
    client
        .send(json!({"id":10,"method":"trust_repository","path":unusual}))
        .await;
    assert_eq!(
        client.receive(false).await["result"],
        json!({"trusted":true})
    );
    assert_eq!(fs::read(&f.gitconfig).unwrap(), trusted_config);
    client
        .send(json!({"id":4,"method":"repository_type","path":unusual.join("nested")}))
        .await;
    let result = client.receive(false).await;
    assert_eq!(result["result"]["kind"], "regular", "{result}");
    client
        .send(json!({"id":5,"method":"repository_type","path":nested}))
        .await;
    assert_eq!(client.receive(false).await["result"]["kind"], "unsafe");
    client.send(json!({"id":2,"method":"snapshot"})).await;
    assert_eq!(client.receive(false).await["result"], Value::Null);
    client.0.close(None).await.unwrap();
    server.kill().await.unwrap();
    server.wait().await.unwrap();
}
