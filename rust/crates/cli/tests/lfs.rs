//! Offline integration coverage for LFS payload diffs and index transactions.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
};

struct Scratch {
    root: PathBuf,
    path: OsString,
}
impl Scratch {
    fn new() -> Option<Self> {
        let supplied = std::env::var_os("GIT_LFS_BIN").map(PathBuf::from);
        let binary = supplied.clone().or_else(|| {
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .map(|dir| {
                    dir.join(if cfg!(windows) {
                        "git-lfs.exe"
                    } else {
                        "git-lfs"
                    })
                })
                .find(|path| path.is_file())
        });
        let Some(binary) = binary else {
            assert!(
                std::env::var_os("DESKTOP_TEST_REQUIRE_LFS").is_none(),
                "Git LFS is required; set GIT_LFS_BIN"
            );
            eprintln!("skipping LFS integration: set GIT_LFS_BIN to git-lfs");
            return None;
        };
        let binary = fs::canonicalize(binary).expect("resolve Git LFS binary");
        assert!(
            Command::new(&binary)
                .arg("version")
                .status()
                .expect("run Git LFS")
                .success(),
            "Git LFS is unavailable"
        );
        let mut paths = vec![binary.parent().unwrap().to_owned()];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        let path = std::env::join_paths(paths).expect("construct test PATH");
        let root = std::env::temp_dir().join(format!("desktop-lfs-diffs-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        Some(Self { root, path })
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct Fixture<'a> {
    scratch: &'a Scratch,
    repo: PathBuf,
    path: OsString,
    exec_path: Option<PathBuf>,
    literal_pathspecs: bool,
}
impl<'a> Fixture<'a> {
    fn new(scratch: &'a Scratch, name: &str, options: &[&str]) -> Self {
        let f = Self::unconfigured(scratch, OsStr::new(name), options);
        assert_eq!(f.rpc(json!({"method":"lfs_install"}), true)["code"], 0);
        f.git(&["lfs", "track", "*.lfs"]);
        f
    }
    fn unconfigured(scratch: &'a Scratch, name: &OsStr, options: &[&str]) -> Self {
        let f = Self {
            scratch,
            repo: scratch.root.join(name),
            path: scratch.path.clone(),
            exec_path: None,
            literal_pathspecs: false,
        };
        let output = f
            .command("git")
            .args(["init", "-q", "-b", "main"])
            .args(options)
            .arg(&f.repo)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        f.git(&["config", "user.name", "LFS Diff Test"]);
        f.git(&["config", "user.email", "lfs@example.invalid"]);
        f.git(&["config", "commit.gpgsign", "false"]);
        f
    }
    fn command(&self, exe: &str) -> Command {
        let mut command = Command::new(exe);
        command
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_SYSTEM")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("GIT_EXEC_PATH")
            .env("PATH", &self.path)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.scratch.root.join("global"))
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", "false")
            .env("SSH_ASKPASS", "false")
            .env("GIT_EDITOR", "true");
        if let Some(path) = &self.exec_path {
            command.env("GIT_EXEC_PATH", path);
        }
        if self.literal_pathspecs {
            command.env("GIT_LITERAL_PATHSPECS", "1");
        }
        command
    }
    fn git_os(&self, args: &[&OsStr], success: bool) -> Output {
        let output = self
            .command("git")
            .arg("-C")
            .arg(&self.repo)
            .args(args)
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            success,
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
    fn git(&self, args: &[&str]) -> Output {
        self.git_os(&args.iter().map(OsStr::new).collect::<Vec<_>>(), true)
    }
    fn rpc(&self, value: Value, success: bool) -> Value {
        let mut child = self
            .command(env!("CARGO_BIN_EXE_desktop-rs"))
            .arg("-C")
            .arg(&self.repo)
            .args(["repo", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(value.to_string().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.success(),
            success,
            "{value}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if !success {
            return Value::Null;
        }
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            result.get("code").and_then(Value::as_i64).unwrap_or(0),
            0,
            "{result}"
        );
        result
    }
    fn cli(&self, args: &[&str], success: bool) -> Output {
        let output = self
            .command(env!("CARGO_BIN_EXE_desktop-rs"))
            .arg("-C")
            .arg(&self.repo)
            .args(args)
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            success,
            "{args:?}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
    #[cfg(unix)]
    fn discard(&self, path: Value) -> String {
        let preview = self.rpc(json!({"method":"discard_preview","path":path}), true);
        let result = self.rpc(
            json!({"method":"discard","path":path,"basis":preview["basis"]}),
            true,
        );
        result["undo_id"].as_str().unwrap().to_owned()
    }
    #[cfg(unix)]
    fn undo(&self, id: &str, success: bool) {
        self.rpc(json!({"method":"undo_discard","undo_id":id}), success);
    }
    fn detail(&self, path: Value, staged: bool) -> Value {
        self.rpc(
            json!({"method":"diff_detail","path":path,"staged":staged}),
            true,
        )
    }
    fn select(&self, path: Value, view: &Value, lines: Vec<Value>, success: bool) {
        self.rpc(json!({"method":"select_diff","path":path,"staged":view["staged"],"basis":view["basis"],"lines":lines}), success);
    }
    fn select_all(&self, path: Value, view: &Value) {
        self.select(path, view, changes(view, None), true);
    }
    fn commit(&self) {
        self.git(&["add", "--all"]);
        self.git(&["commit", "-qm", "checkpoint"]);
    }
    fn media(&self) -> PathBuf {
        let result = self.git(&["lfs", "env"]);
        let bytes = result
            .stdout
            .split(|b| *b == b'\n')
            .find_map(|line| line.strip_prefix(b"LocalMediaDir="))
            .expect("LFS media directory");
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            PathBuf::from(OsString::from_vec(bytes.to_vec()))
        }
        #[cfg(not(unix))]
        {
            PathBuf::from(String::from_utf8(bytes.to_vec()).unwrap())
        }
    }
    fn cache(&self, data: &[u8]) -> PathBuf {
        let oid = format!("{:x}", Sha256::digest(data));
        self.media().join(&oid[..2]).join(&oid[2..4]).join(&oid)
    }
    fn index_os(&self, name: &OsStr) -> Vec<u8> {
        let mut arg = OsString::from(":");
        arg.push(name);
        let output = self.git_os(&[OsStr::new("show"), &arg], true).stdout;
        if output.is_empty() {
            return Vec::new();
        }
        let text = String::from_utf8(output).unwrap();
        let oid = text
            .lines()
            .find_map(|line| line.strip_prefix("oid sha256:"))
            .expect("LFS pointer");
        fs::read(self.media().join(&oid[..2]).join(&oid[2..4]).join(oid)).unwrap()
    }
    fn index(&self, name: &str) -> Vec<u8> {
        self.index_os(OsStr::new(name))
    }
}
fn changes(view: &Value, hunk: Option<usize>) -> Vec<Value> {
    view["hunks"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .filter(|(index, _)| hunk.is_none_or(|wanted| wanted == *index))
        .flat_map(|(_, hunk)| hunk["lines"].as_array().unwrap())
        .filter(|line| line["kind"] == "added" || line["kind"] == "deleted")
        .map(|line| line["id"].clone())
        .collect()
}

#[cfg(unix)]
#[test]
fn lfs_setup_and_status_preserve_hooks_and_configuration() {
    use std::os::unix::fs::PermissionsExt;
    let Some(scratch) = Scratch::new() else {
        return;
    };
    let f = Fixture::unconfigured(&scratch, OsStr::new("inspection"), &[]);
    let hooks = f.repo.join(".git/hooks/pre-push");
    let local = f.repo.join(".git/config");
    let before = fs::read(&local).unwrap();
    let status = f.rpc(json!({"method":"lfs_status"}), true);
    assert_eq!(status["available"], true);
    assert_eq!(status["using_lfs"], false);
    assert!(!hooks.exists());
    assert_eq!(fs::read(&local).unwrap(), before);
    let attributes = f.repo.join(".gitattributes");
    fs::write(
        &attributes,
        "*.bin filter=lfs lockable\n*.excluded -filter\n",
    )
    .unwrap();
    let status = f.rpc(
        json!({"method":"lfs_status","paths":["payload.bin","payload.excluded"]}),
        true,
    );
    assert_eq!(status["using_lfs"], true);
    assert_eq!(status["files"][0]["tracked"], true);
    assert_eq!(status["files"][1]["tracked"], false);
    assert_eq!(
        status["patterns"],
        json!([
            {"pattern":"*.bin","source":".gitattributes","lockable":true,"tracked":true},
            {"pattern":"*.excluded","source":".gitattributes","lockable":false,"tracked":false}
        ])
    );
    assert_eq!(fs::read(&local).unwrap(), before);
    assert!(!hooks.exists());
    assert!(!f.repo.join(".git/lfs").exists());
    assert_eq!(
        fs::read_to_string(&attributes).unwrap(),
        "*.bin filter=lfs lockable\n*.excluded -filter\n"
    );
    fs::remove_file(&attributes).unwrap();
    assert_eq!(f.rpc(json!({"method":"lfs_install"}), true)["code"], 0);
    assert!(hooks.exists());
    assert_eq!(
        f.git(&["config", "--local", "filter.lfs.required"]).stdout,
        b"true\n"
    );
    assert!(!scratch.root.join("global").exists());
    f.git(&["lfs", "track", "*.bin"]);
    fs::write(f.repo.join("data.bin"), b"committed LFS body\n").unwrap();
    f.commit();
    let status = f.rpc(json!({"method":"lfs_status","paths":["data.bin","data.bin.txt",{"bytes":[255,46,98,105,110]}]}), true);
    assert_eq!(status["using_lfs"], true);
    assert_eq!(
        status["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["tracked"].clone())
            .collect::<Vec<_>>(),
        vec![json!(true), json!(false), json!(true)]
    );
    assert_eq!(status["patterns"][0]["source"], ".gitattributes");
    fs::remove_file(&hooks).unwrap();
    f.rpc(json!({"method":"lfs_status"}), true);
    assert!(!hooks.exists());
    f.rpc(json!({"method":"lfs_status","paths":["../outside"]}), false);
    f.rpc(json!({"method":"lfs_install","scope":"system"}), false);
    fs::write(&hooks, "#!/bin/sh\necho custom-hook\n").unwrap();
    fs::set_permissions(&hooks, fs::Permissions::from_mode(0o700)).unwrap();
    let failed = f.cli(
        &["repo", &json!({"method":"lfs_install"}).to_string()],
        false,
    );
    let failed: Value = serde_json::from_slice(&failed.stdout).unwrap();
    assert_ne!(failed["code"], 0);
    assert!(fs::read_to_string(&hooks).unwrap().contains("custom-hook"));
    assert_eq!(
        f.rpc(json!({"method":"lfs_install","force":true}), true)["code"],
        0
    );
    assert!(
        fs::read_to_string(&hooks)
            .unwrap()
            .contains("git lfs pre-push")
    );
    f.cli(
        &["lfs", "install", "--global", "--skip-repo", "--skip-smudge"],
        true,
    );
    assert!(
        String::from_utf8(f.git(&["config", "--global", "filter.lfs.smudge"]).stdout)
            .unwrap()
            .contains("--skip")
    );
    fs::remove_file(scratch.root.join("global")).unwrap();
}

#[cfg(unix)]
#[test]
fn lfs_global_install_preserves_private_hooks_and_custom_filters() {
    use std::os::unix::fs::PermissionsExt;
    let Some(scratch) = Scratch::new() else {
        return;
    };
    let f = Fixture::unconfigured(&scratch, OsStr::new("global-hooks"), &[]);
    let hook = f.repo.join(".git/hooks/pre-push");
    let secret = "synthetic-hook-secret";
    fs::write(&hook, format!("#!/bin/sh\necho {secret}\n")).unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
    let result = f.cli(
        &[
            "repo",
            &json!({"method":"lfs_install","scope":"global"}).to_string(),
        ],
        false,
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stdout).unwrap()["error_kind"],
        "configuration"
    );
    assert!(fs::read_to_string(&hook).unwrap().contains(secret));
    assert!(!String::from_utf8_lossy(&result.stdout).contains(secret));
    assert!(!String::from_utf8_lossy(&result.stderr).contains(secret));
    assert_eq!(
        f.git(&["config", "--global", "filter.lfs.required"]).stdout,
        b"true\n"
    );
    assert_eq!(
        f.rpc(
            json!({"method":"lfs_install","scope":"global","force":true}),
            true
        )["code"],
        0
    );
    let updated = fs::read_to_string(&hook).unwrap();
    assert!(updated.contains("git lfs pre-push") && !updated.contains(secret));
    fs::remove_file(scratch.root.join("global")).unwrap();

    let outside = scratch.root.join("not a repository");
    fs::create_dir(&outside).unwrap();
    let secret = "custom-clean-synthetic-filter-secret -- %f";
    f.git(&["config", "--global", "filter.lfs.clean", secret]);
    let install = |force: bool| {
        let mut command = f.command(env!("CARGO_BIN_EXE_desktop-rs"));
        command
            .arg("-C")
            .arg(&outside)
            .args(["lfs", "install", "--global", "--skip-repo"]);
        if force {
            command.arg("--force");
        }
        command.output().unwrap()
    };
    let failed = install(false);
    assert!(!failed.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&failed.stdout).unwrap()["error_kind"],
        "configuration"
    );
    assert!(!String::from_utf8_lossy(&failed.stdout).contains(secret));
    assert!(!String::from_utf8_lossy(&failed.stderr).contains(secret));
    assert_eq!(
        String::from_utf8(f.git(&["config", "--global", "filter.lfs.clean"]).stdout)
            .unwrap()
            .trim(),
        secret
    );
    let forced = install(true);
    assert!(
        forced.status.success(),
        "{}",
        String::from_utf8_lossy(&forced.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&forced.stdout).unwrap()["code"],
        0
    );
    assert_eq!(
        f.git(&["config", "--global", "filter.lfs.clean"]).stdout,
        b"git-lfs clean -- %f\n"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn lfs_standalone_and_empty_bridge_work_without_a_repository() {
    use futures_util::{SinkExt, StreamExt};
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::time::timeout;
    use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
    let Some(scratch) = Scratch::new() else {
        return;
    };
    let f = Fixture::unconfigured(&scratch, OsStr::new("command-fixture"), &[]);
    let outside = scratch.root.join("not a repository");
    fs::create_dir(&outside).unwrap();
    let global = scratch.root.join("global");
    let cli = |args: &[&str], success: bool| {
        let result = f
            .command(env!("CARGO_BIN_EXE_desktop-rs"))
            .arg("-C")
            .arg(&outside)
            .args(args)
            .output()
            .unwrap();
        assert_eq!(
            result.status.success(),
            success,
            "{args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        serde_json::from_slice::<Value>(&result.stdout).unwrap_or(Value::Null)
    };
    assert_eq!(cli(&["lfs", "tool-status"], true)["available"], true);
    assert_eq!(
        cli(
            &["repo", &json!({"method":"lfs_tool_status"}).to_string()],
            true
        )["available"],
        true
    );
    assert_eq!(cli(&["lfs"], true)["available"], true);
    assert!(!global.exists());
    cli(&["lfs", "install", "--global"], false);
    assert!(!global.exists());
    assert_eq!(
        cli(
            &["lfs", "install", "--global", "--skip-repo", "--skip-smudge"],
            true
        )["code"],
        0
    );
    assert!(
        String::from_utf8(f.git(&["config", "--global", "filter.lfs.smudge"]).stdout)
            .unwrap()
            .contains("--skip")
    );
    assert_eq!(
        cli(
            &[
                "repo",
                &json!({"method":"lfs_install","scope":"global","skip_repo":true,"force":true})
                    .to_string()
            ],
            true
        )["code"],
        0
    );
    fs::remove_file(&global).unwrap();

    let mut server = tokio::process::Command::new(env!("CARGO_BIN_EXE_desktop-rs"))
        .arg("-C")
        .arg(&outside)
        .arg("--config")
        .arg(scratch.root.join("registry.json"))
        .args(["serve", "--empty", "--port", "0"])
        .current_dir(&outside)
        .env("HOME", &scratch.root)
        .env("PATH", &scratch.path)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", &global)
        .env("GIT_TERMINAL_PROMPT", "0")
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
    let mut lines = BufReader::new(server.stderr.take().unwrap()).lines();
    let line = timeout(Duration::from_secs(10), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let (origin, token) = line
        .strip_prefix("Open ")
        .unwrap()
        .split_once("/#token=")
        .unwrap();
    let authority = origin.strip_prefix("http://").unwrap();
    let mut request = format!("ws://{authority}/ws")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("Origin", origin.parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    socket
        .send(Message::text(json!({"token":token}).to_string()))
        .await
        .unwrap();
    let ready = timeout(Duration::from_secs(10), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(ready.to_text().unwrap()).unwrap()["ready"],
        true
    );
    for request in [
        json!({"id":1,"method":"lfs_status"}),
        json!({"id":2,"method":"lfs_install","scope":"global","skip_repo":true}),
        json!({"id":3,"method":"lfs_install","scope":"local"}),
        json!({"id":4,"method":"lfs_checkout"}),
        json!({"id":5,"method":"lfs_status","paths":["file"]}),
        json!({"id":6,"method":"snapshot"}),
        json!({"id":7,"method":"lfs_tool_status","repository_id":"an obsolete repository"}),
    ] {
        socket
            .send(Message::text(request.to_string()))
            .await
            .unwrap();
        let message = timeout(Duration::from_secs(10), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let reply: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
        assert_eq!(reply["id"], request["id"]);
        match request["id"].as_u64().unwrap() {
            1 | 7 => assert_eq!(reply["result"]["available"], true),
            2 => assert_eq!(reply["result"]["code"], 0),
            3..=5 => assert!(reply.get("error").is_some(), "{reply}"),
            6 => assert!(reply["result"].is_null()),
            _ => unreachable!(),
        }
    }
    socket.close(None).await.unwrap();
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

#[cfg(unix)]
#[test]
fn lfs_tracking_preserves_index_permissions_and_attribute_boundaries() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let Some(scratch) = Scratch::new() else {
        return;
    };
    let f = Fixture::unconfigured(&scratch, OsStr::new("lock-rules"), &[]);
    f.rpc(json!({"method":"lfs_install"}), true);
    let file = f.repo.join("file.asset");
    fs::write(&file, b"existing regular Git content").unwrap();
    f.commit();
    let original = f.git(&["show", ":file.asset"]).stdout;
    assert_eq!(
        f.rpc(
            json!({"method":"lfs_track","patterns":["*.asset"],"lockable":true}),
            true
        )["code"],
        0
    );
    assert_eq!(fs::read(&file).unwrap(), original);
    assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o222, 0);
    assert_eq!(f.git(&["show", ":file.asset"]).stdout, original);
    assert_eq!(
        f.rpc(
            json!({"method":"lfs_track","patterns":["*.asset"],"lockable":false}),
            true
        )["code"],
        0
    );
    assert_ne!(fs::metadata(&file).unwrap().permissions().mode() & 0o200, 0);
    assert_eq!(
        f.rpc(json!({"method":"lfs_untrack","patterns":["*.asset"]}), true)["code"],
        0
    );

    let f = Fixture::unconfigured(&scratch, OsStr::new("tracking"), &[]);
    f.rpc(json!({"method":"lfs_install"}), true);
    let attributes = f.repo.join(".gitattributes");
    fs::write(&attributes, "# preserve this comment\nordinary text=auto\n").unwrap();
    f.commit();
    let result = f.cli(
        &["lfs", "track", "--filename", "--", "literal[1]*.bin"],
        true,
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stdout).unwrap()["code"],
        0
    );
    let status = f.rpc(
        json!({"method":"lfs_status","paths":["literal[1]*.bin","literal11foo.bin"]}),
        true,
    );
    assert_eq!(status["files"][0]["tracked"], true);
    assert_eq!(status["files"][1]["tracked"], false);
    let text = fs::read_to_string(&attributes).unwrap();
    assert!(text.contains("# preserve this comment") && text.contains("ordinary text=auto"));
    assert!(
        f.git(&["diff", "--cached", "--name-only"])
            .stdout
            .is_empty()
    );
    assert_eq!(
        f.rpc(
            json!({"method":"lfs_track","patterns":["-leading.bin"]}),
            true
        )["code"],
        0
    );
    assert_eq!(
        f.rpc(
            json!({"method":"lfs_status","paths":["-leading.bin"]}),
            true
        )["files"][0]["tracked"],
        true
    );
    assert_eq!(
        f.rpc(
            json!({"method":"lfs_untrack","patterns":["-leading.bin"]}),
            true
        )["code"],
        0
    );
    assert_eq!(
        f.rpc(
            json!({"method":"lfs_status","paths":["-leading.bin"]}),
            true
        )["files"][0]["tracked"],
        false
    );
    for patterns in [json!(["../outside"]), json!(["bad\npattern"]), json!([])] {
        f.rpc(json!({"method":"lfs_track","patterns":patterns}), false);
    }
    let previous = fs::read(&attributes).unwrap();
    fs::remove_file(&attributes).unwrap();
    let sentinel = scratch.root.join("outside-attributes");
    fs::write(&sentinel, b"outside").unwrap();
    symlink(&sentinel, &attributes).unwrap();
    f.rpc(json!({"method":"lfs_track","patterns":["*.bad"]}), false);
    assert_eq!(fs::read(&sentinel).unwrap(), b"outside");
    fs::remove_file(&attributes).unwrap();
    fs::write(attributes, previous).unwrap();
}

#[cfg(unix)]
fn copy_tree(source: &std::path::Path, target: &std::path::Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let destination = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &destination);
        } else {
            fs::copy(entry.path(), destination).unwrap();
        }
    }
}

#[cfg(unix)]
#[test]
fn lfs_discard_undo_preserves_payloads_and_cache_boundaries() {
    use std::os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::symlink,
    };
    let Some(scratch) = Scratch::new() else {
        return;
    };
    for label in [
        "standard",
        "sha256",
        "custom-storage",
        "linked",
        "raw-path",
        "noncanonical",
        "raw-repository",
    ] {
        let raw_repo = OsString::from_vec(b"\xfe-repository".to_vec());
        let repo_name = if label == "raw-repository" {
            raw_repo.as_os_str()
        } else {
            OsStr::new(label)
        };
        let options: &[&str] = if label == "sha256" {
            &["--object-format=sha256"]
        } else {
            &[]
        };
        let mut f = Fixture::unconfigured(&scratch, repo_name, options);
        assert_eq!(f.rpc(json!({"method":"lfs_install"}), true)["code"], 0);
        if label == "custom-storage" {
            f.git(&[
                "config",
                "lfs.storage",
                scratch.root.join("LFS store with spaces").to_str().unwrap(),
            ]);
        }
        f.git(&["lfs", "track", "*.bin"]);
        let name = if label == "raw-path" {
            OsStr::from_bytes(b"\xff.bin")
        } else {
            OsStr::new("large.bin")
        };
        let path = if label == "raw-path" {
            json!({"bytes":name.as_bytes()})
        } else {
            json!("large.bin")
        };
        let mut file = f.repo.join(name);
        fs::write(&file, b"head\0binary\n").unwrap();
        f.commit();
        if label == "linked" {
            let linked = scratch.root.join("linked-tree");
            f.git(&["worktree", "add", "-qb", "linked", linked.to_str().unwrap()]);
            f.repo = linked;
            file = f.repo.join(name);
        }
        let staged = b"staged payload\0\n".repeat(if label == "standard" { 300_000 } else { 300 });
        let working = b"working payload\n";
        fs::write(&file, &staged).unwrap();
        f.git_os(&[OsStr::new("add"), OsStr::new("--"), name], true);
        let mut index_name = OsString::from(":");
        index_name.push(name);
        let mut saved_pointer = f.git_os(&[OsStr::new("show"), &index_name], true).stdout;
        if label == "noncanonical" {
            let text = String::from_utf8(saved_pointer)
                .unwrap()
                .trim_end_matches('\n')
                .replace('\n', "\r\n")
                .replace("size ", "size +0");
            saved_pointer = text.into_bytes();
            let candidate = scratch.root.join("noncanonical-pointer");
            fs::write(&candidate, &saved_pointer).unwrap();
            f.git(&[
                "lfs",
                "pointer",
                "--check",
                &format!("--file={}", candidate.display()),
            ]);
            let blob = String::from_utf8(
                f.git(&[
                    "hash-object",
                    "-w",
                    "--no-filters",
                    "--",
                    candidate.to_str().unwrap(),
                ])
                .stdout,
            )
            .unwrap();
            f.git_os(
                &[
                    OsStr::new("update-index"),
                    OsStr::new("--cacheinfo"),
                    OsStr::new("100644"),
                    OsStr::new(blob.trim()),
                    name,
                ],
                true,
            );
        }
        let text = String::from_utf8(saved_pointer.clone()).unwrap();
        let oid = text
            .lines()
            .find_map(|line| line.strip_prefix("oid sha256:"))
            .unwrap();
        let mut media = f.media();
        let mut cache = media.join(&oid[..2]).join(&oid[2..4]).join(oid);
        assert_eq!(fs::read(&cache).unwrap(), staged);
        fs::write(&file, working).unwrap();
        let plan = f.rpc(json!({"method":"discard_preview", "path":path}), true);
        assert!(plan["backup_bytes"].as_u64().unwrap() >= (staged.len() + working.len()) as u64);
        let result = f.rpc(
            json!({"method":"discard", "path":path, "basis":plan["basis"]}),
            true,
        );
        let undo_id = result["undo_id"].as_str().unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"head\0binary\n");
        let gitdir = f.git(&["rev-parse", "--absolute-git-dir"]).stdout;
        let gitdir = PathBuf::from(OsString::from_vec(
            gitdir.strip_suffix(b"\n").unwrap().to_vec(),
        ));
        let journal = gitdir.join("desktop-undo").join(undo_id);
        assert_eq!(
            fs::read(journal.join(format!("lfs-{oid}"))).unwrap(),
            staged
        );
        fs::remove_file(&cache).unwrap();
        f.git(&["prune", "--expire", "now"]);
        if label == "custom-storage" {
            let moved = scratch.root.join("Moved LFS storage");
            copy_tree(&media, &moved.join("objects"));
            f.git(&["config", "lfs.storage", moved.to_str().unwrap()]);
            media = moved.join("objects");
            cache = media.join(&oid[..2]).join(&oid[2..4]).join(oid);
            assert!(!cache.exists());
        }
        f.undo(undo_id, true);
        assert_eq!(fs::read(&file).unwrap(), working);
        assert_eq!(
            f.git_os(&[OsStr::new("show"), &index_name], true).stdout,
            saved_pointer
        );
        assert_eq!(fs::read(&cache).unwrap(), staged);
        fs::write(&file, &saved_pointer).unwrap();
        f.git_os(&[OsStr::new("lfs"), OsStr::new("checkout"), name], true);
        assert_eq!(fs::read(&file).unwrap(), staged);
        fs::write(&file, working).unwrap();
        fs::remove_file(&cache).unwrap();
        let before = fs::read(gitdir.join("index")).unwrap();
        f.rpc(json!({"method":"discard_preview","path":path}), false);
        assert_eq!(fs::read(&file).unwrap(), working);
        assert_eq!(fs::read(gitdir.join("index")).unwrap(), before);
        fs::write(&cache, b"corrupt local payload").unwrap();
        f.rpc(json!({"method":"discard_preview","path":path}), false);
        fs::write(&cache, &staged).unwrap();
        let undo_id = f.discard(path.clone());
        let before = fs::read(gitdir.join("index")).unwrap();
        let saved = gitdir
            .join("desktop-undo")
            .join(&undo_id)
            .join(format!("lfs-{oid}"));
        fs::write(&saved, b"corrupt backup").unwrap();
        f.undo(&undo_id, false);
        assert_eq!(fs::read(&file).unwrap(), b"head\0binary\n");
        assert_eq!(fs::read(gitdir.join("index")).unwrap(), before);
        fs::write(&saved, &staged).unwrap();
        fs::write(&cache, b"corrupt destination").unwrap();
        f.undo(&undo_id, false);
        assert_eq!(fs::read(&file).unwrap(), b"head\0binary\n");
        fs::remove_file(&cache).unwrap();
        let outside = scratch.root.join(format!("outside-{label}"));
        fs::write(&outside, b"untouched").unwrap();
        symlink(&outside, &cache).unwrap();
        f.undo(&undo_id, false);
        assert_eq!(fs::read(outside).unwrap(), b"untouched");
        assert_eq!(fs::read(&file).unwrap(), b"head\0binary\n");
        fs::remove_file(&cache).unwrap();
        f.undo(&undo_id, true);
        assert_eq!(fs::read(&file).unwrap(), working);
        assert_eq!(fs::read(&cache).unwrap(), staged);
    }
}

#[cfg(unix)]
#[test]
fn lfs_empty_pointer_and_missing_executable() {
    use std::os::unix::fs::PermissionsExt;
    let Some(scratch) = Scratch::new() else {
        return;
    };
    let mut f = Fixture::unconfigured(&scratch, OsStr::new("empty-pointer"), &[]);
    f.rpc(json!({"method":"lfs_install"}), true);
    f.git(&["lfs", "track", "*.bin"]);
    let file = f.repo.join("empty.bin");
    fs::write(&file, b"head body\n").unwrap();
    f.commit();
    let empty = pointer(b"");
    let saved = scratch.root.join("empty-pointer-blob");
    fs::write(&saved, &empty).unwrap();
    let blob = String::from_utf8(
        f.git(&[
            "hash-object",
            "-w",
            "--no-filters",
            "--",
            saved.to_str().unwrap(),
        ])
        .stdout,
    )
    .unwrap();
    f.git(&[
        "update-index",
        "--cacheinfo",
        "100644",
        blob.trim(),
        "empty.bin",
    ]);
    fs::write(&file, b"working body\n").unwrap();
    let undo = f.discard(json!("empty.bin"));
    f.undo(&undo, true);
    assert_eq!(fs::read(&file).unwrap(), b"working body\n");
    assert_eq!(f.git(&["show", ":empty.bin"]).stdout, empty.as_bytes());
    fs::write(&file, &empty).unwrap();
    f.git(&["lfs", "checkout", "empty.bin"]);
    assert!(fs::read(&file).unwrap().is_empty());
    let fake = scratch.root.join("missing-lfs-empty");
    fs::create_dir(&fake).unwrap();
    let tool = fake.join("git-lfs");
    fs::write(&tool, "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o700)).unwrap();
    let mut paths = vec![fake.clone()];
    paths.extend(std::env::split_paths(&f.path));
    f.path = std::env::join_paths(paths).unwrap();
    f.exec_path = Some(fake);
    let status = f.rpc(json!({"method":"lfs_status", "paths":["empty.bin"]}), true);
    assert_eq!(status["available"], false);
    assert_eq!(status["files"][0]["tracked"], true);
    assert!(status.get("hint").is_some());
}

struct OfflineProbe {
    url: String,
    contacts: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl OfflineProbe {
    fn new() -> Self {
        use std::{
            net::TcpListener,
            sync::{
                Arc,
                atomic::{AtomicBool, AtomicUsize, Ordering},
            },
            thread,
            time::Duration,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/objects", listener.local_addr().unwrap());
        let contacts = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_contacts = contacts.clone();
        let thread_stop = stop.clone();
        let worker = thread::spawn(move || {
            loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        thread_contacts.fetch_add(1, Ordering::Relaxed);
                        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if thread_stop.load(Ordering::Acquire) {
                            break;
                        }
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("offline probe failed: {error}"),
                }
            }
        });
        Self {
            url,
            contacts,
            stop,
            worker: Some(worker),
        }
    }
    fn assert_offline(&self) {
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(self.contacts.load(std::sync::atomic::Ordering::Acquire), 0);
    }
    fn finish(mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
        assert_eq!(
            self.contacts.load(std::sync::atomic::Ordering::Acquire),
            0,
            "LFS preview contacted the server"
        );
    }
}
impl Drop for OfflineProbe {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[test]
fn lfs_diff_transactions_and_cache_safety() {
    let Some(scratch) = Scratch::new() else {
        return;
    };
    let f = Fixture::new(&scratch, "text", &[]);
    let file = f.repo.join("data.lfs");
    let base = (0..60).map(|n| format!("line {n}\n")).collect::<String>();
    let modified = base
        .replace("line 2\n", "changed two\n")
        .replace("line 50\n", "changed fifty\n");
    fs::write(&file, &base).unwrap();
    f.commit();
    fs::write(&file, &modified).unwrap();
    fs::write(f.repo.join("unrelated"), b"keep staged").unwrap();
    f.git(&["add", "--", "unrelated"]);
    let unrelated = f.git(&["ls-files", "--stage", "--", "unrelated"]).stdout;
    let view = f.detail(json!("data.lfs"), false);
    assert_eq!(view["kind"], "text");
    assert_eq!(view["can_select"], true);
    assert_eq!(view["hunks"].as_array().unwrap().len(), 2);
    assert_eq!(view["before"]["lfs"]["state"], "available");
    assert_eq!(view["before"]["size"], base.len());
    assert!(
        view["hunks"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|h| h["lines"].as_array().unwrap())
            .all(|line| !line["text"].as_str().unwrap().contains("version https:"))
    );
    let first = changes(&view, Some(0));
    f.select(json!("data.lfs"), &view, first.clone(), true);
    assert_eq!(
        f.index("data.lfs"),
        base.replace("line 2\n", "changed two\n").as_bytes()
    );
    assert_eq!(fs::read(&file).unwrap(), modified.as_bytes());
    assert_eq!(
        f.git(&["ls-files", "--stage", "--", "unrelated"]).stdout,
        unrelated
    );
    f.select(json!("data.lfs"), &view, first, false);
    f.select_all(json!("data.lfs"), &f.detail(json!("data.lfs"), false));
    assert_eq!(f.index("data.lfs"), modified.as_bytes());
    let staged = f.detail(json!("data.lfs"), true);
    assert_eq!(staged["before"]["lfs"]["state"], "available");
    assert_eq!(staged["after"]["lfs"]["state"], "available");
    f.select(json!("data.lfs"), &staged, changes(&staged, Some(0)), true);
    assert_eq!(
        f.index("data.lfs"),
        base.replace("line 50\n", "changed fifty\n").as_bytes()
    );
    f.select_all(json!("data.lfs"), &f.detail(json!("data.lfs"), true));
    assert_eq!(f.index("data.lfs"), base.as_bytes());
    assert_eq!(fs::read(&file).unwrap(), modified.as_bytes());

    let view = f.detail(json!("data.lfs"), false);
    let selection = json!({"basis":view["basis"],"lines":changes(&view, Some(0))});
    let preview = f.rpc(json!({"method":"discard_preview","path":"data.lfs","scope":"working_tree","selection":selection}), true);
    let undo = f.rpc(json!({"method":"discard","path":"data.lfs","scope":"working_tree","selection":selection,"basis":preview["basis"]}), true);
    assert_eq!(
        fs::read(&file).unwrap(),
        base.replace("line 50\n", "changed fifty\n").as_bytes()
    );
    assert_eq!(f.index("data.lfs"), base.as_bytes());
    f.rpc(
        json!({"method":"undo_discard","undo_id":undo["undo_id"]}),
        true,
    );
    assert_eq!(fs::read(&file).unwrap(), modified.as_bytes());
    let view = f.detail(json!("data.lfs"), false);
    let before = fs::read(f.repo.join(".git/index")).unwrap();
    f.git(&["config", "filter.lfs.smudge", "git-lfs smudge --skip -- %f"]);
    f.select(json!("data.lfs"), &view, changes(&view, None), false);
    assert_eq!(fs::read(f.repo.join(".git/index")).unwrap(), before);
    f.rpc(json!({"method":"lfs_install","force":true}), true);
    f.git(&["config", "--unset", "filter.lfs.process"]);
    f.git(&["config", "filter.lfs.clean", "cat"]);
    let view = f.detail(json!("data.lfs"), false);
    f.select(json!("data.lfs"), &view, changes(&view, None), false);
    assert_eq!(fs::read(f.repo.join(".git/index")).unwrap(), before);
    assert_eq!(fs::read(&file).unwrap(), modified.as_bytes());
    f.rpc(json!({"method":"lfs_install","force":true}), true);
    f.select_all(json!("data.lfs"), &f.detail(json!("data.lfs"), false));
    f.git(&["commit", "-qm", "modified LFS"]);
    let history = f.rpc(
        json!({"method":"commit_diff","path":"data.lfs","commit":"HEAD"}),
        true,
    );
    assert_eq!(history["kind"], "text");
    assert_eq!(history["hunks"].as_array().unwrap().len(), 2);
    assert_eq!(history["can_select"], false);

    let server = OfflineProbe::new();
    f.git(&["config", "lfs.url", &server.url]);
    let cache = f.cache(modified.as_bytes());
    fs::remove_file(&cache).unwrap();
    fs::write(&file, format!("{modified}working\n")).unwrap();
    let before_index = fs::read(f.repo.join(".git/index")).unwrap();
    let working = fs::read(&file).unwrap();
    for request in [
        json!({"method":"diff_detail","path":"data.lfs"}),
        json!({"method":"diff_detail","path":"data.lfs","staged":true}),
        json!({"method":"commit_diff","path":"data.lfs","commit":"HEAD"}),
    ] {
        let missing = f.rpc(request, true);
        assert_eq!(missing["kind"], "lfs_unavailable");
        assert_eq!(missing["hunks"], json!([]));
        assert_eq!(missing["can_select"], false);
        assert!(!cache.exists());
        server.assert_offline();
    }
    assert_eq!(fs::read(f.repo.join(".git/index")).unwrap(), before_index);
    assert_eq!(fs::read(&file).unwrap(), working);
    fs::write(&cache, vec![b'x'; modified.len()]).unwrap();
    let corrupt = f.detail(json!("data.lfs"), false);
    assert_eq!(corrupt["before"]["lfs"]["state"], "corrupt");
    assert_eq!(corrupt["kind"], "lfs_unavailable");
    fs::remove_file(&cache).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let outside = scratch.root.join("outside");
        fs::write(&outside, modified.as_bytes()).unwrap();
        symlink(&outside, &cache).unwrap();
        assert_eq!(
            f.detail(json!("data.lfs"), false)["before"]["lfs"]["state"],
            "unavailable"
        );
        assert_eq!(fs::read(&outside).unwrap(), modified.as_bytes());
        fs::remove_file(&cache).unwrap();
        assert!(
            Command::new("mkfifo")
                .arg(&cache)
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            f.detail(json!("data.lfs"), false)["kind"],
            "lfs_unavailable"
        );
        fs::remove_file(&cache).unwrap();
    }
    fs::write(&cache, modified.as_bytes()).unwrap();
    server.finish();

    fs::remove_file(&file).unwrap();
    f.select_all(json!("data.lfs"), &f.detail(json!("data.lfs"), false));
    assert!(f.git(&["ls-files", "--", "data.lfs"]).stdout.is_empty());
    f.select_all(json!("data.lfs"), &f.detail(json!("data.lfs"), true));
    assert_eq!(f.index("data.lfs"), modified.as_bytes());
    assert!(!file.exists());
    fs::write(&file, b"").unwrap();
    f.select_all(json!("data.lfs"), &f.detail(json!("data.lfs"), false));
    assert!(f.git(&["show", ":data.lfs"]).stdout.is_empty());
    f.select_all(json!("data.lfs"), &f.detail(json!("data.lfs"), true));
    assert_eq!(f.index("data.lfs"), modified.as_bytes());
    fs::write(&file, &modified).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        for name in [
            b"new.lfs".as_slice(),
            b":(glob)*.lfs",
            b"-dash.lfs",
            b"line\nbreak.lfs",
            b"\xff.lfs",
        ] {
            let target = f.repo.join(OsStr::from_bytes(name));
            fs::write(&target, b"one\r\ntwo\r\nlast").unwrap();
            let path = json!({"bytes":name});
            let view = f.detail(path.clone(), false);
            assert_eq!(view["can_select"], true);
            let ids = changes(&view, None);
            f.select(
                path.clone(),
                &view,
                vec![ids[0].clone(), ids.last().unwrap().clone()],
                true,
            );
            assert_eq!(f.index_os(OsStr::from_bytes(name)), b"one\r\nlast");
            assert_eq!(fs::read(target).unwrap(), b"one\r\ntwo\r\nlast");
            f.select_all(path.clone(), &f.detail(path, true));
            assert!(
                f.git_os(
                    &[
                        OsStr::new("--literal-pathspecs"),
                        OsStr::new("ls-files"),
                        OsStr::new("--"),
                        OsStr::from_bytes(name)
                    ],
                    true
                )
                .stdout
                .is_empty()
            );
        }
    }
}

#[test]
fn lfs_diff_uses_the_right_cache_and_index() {
    let Some(scratch) = Scratch::new() else {
        return;
    };
    for label in ["sha256", "linked", "split", "storage"] {
        let options: &[&str] = if label == "sha256" {
            &["--object-format=sha256"]
        } else {
            &[]
        };
        let mut f = Fixture::new(&scratch, label, options);
        if label == "storage" {
            f.git(&[
                "config",
                "lfs.storage",
                scratch.root.join("custom store").to_str().unwrap(),
            ]);
        }
        let mut target = f.repo.join("data.lfs");
        fs::write(&target, b"before\nsecond\n").unwrap();
        f.commit();
        if label == "linked" {
            let linked = scratch.root.join("worktree");
            f.git(&["worktree", "add", "-qb", "linked", linked.to_str().unwrap()]);
            f.repo = linked;
            target = f.repo.join("data.lfs");
        }
        if label == "split" {
            f.git(&["config", "core.splitIndex", "true"]);
            f.git(&["update-index", "--split-index"]);
        }
        fs::write(&target, b"after\nsecond\n").unwrap();
        f.select_all(json!("data.lfs"), &f.detail(json!("data.lfs"), false));
        assert_eq!(f.index("data.lfs"), fs::read(&target).unwrap(), "{label}");
    }
}

#[test]
fn lfs_conflict_previews_do_not_repair_missing_objects() {
    let Some(scratch) = Scratch::new() else {
        return;
    };
    let f = Fixture::new(&scratch, "conflict", &[]);
    let target = f.repo.join("data.lfs");
    fs::write(&target, b"base\nsecond\n").unwrap();
    f.commit();
    f.git(&["switch", "-qc", "side"]);
    fs::write(&target, b"side\nsecond\n").unwrap();
    f.commit();
    f.git(&["switch", "-q", "main"]);
    fs::write(&target, b"main\nsecond\n").unwrap();
    f.commit();
    f.git_os(&[OsStr::new("merge"), OsStr::new("side")], false);
    let detail = f.rpc(json!({"method":"conflict_detail","path":"data.lfs"}), true);
    let stages = detail["stages"].as_array().unwrap();
    assert_eq!(
        stages
            .iter()
            .map(|stage| stage["text"].clone())
            .collect::<Vec<_>>(),
        vec![
            json!("base\nsecond\n"),
            json!("main\nsecond\n"),
            json!("side\nsecond\n")
        ]
    );
    assert!(
        stages
            .iter()
            .all(|stage| stage["preview"]["lfs"]["state"] == "available")
    );
    let side = f.cache(b"side\nsecond\n");
    fs::remove_file(&side).unwrap();
    let before = fs::read(f.repo.join(".git/index")).unwrap();
    let detail = f.rpc(json!({"method":"conflict_detail","path":"data.lfs"}), true);
    assert!(detail["stages"][2]["text"].is_null());
    assert_eq!(detail["stages"][2]["preview"]["lfs"]["state"], "missing");
    assert!(!side.exists());
    assert_eq!(fs::read(f.repo.join(".git/index")).unwrap(), before);
    fs::write(side, b"side\nsecond\n").unwrap();
    f.rpc(json!({"method":"resolve_conflict","path":"data.lfs","choice":"ours","basis":detail["basis"]}), true);
    assert_eq!(fs::read(target).unwrap(), b"main\nsecond\n");
    assert_eq!(f.index("data.lfs"), b"main\nsecond\n");
    assert!(f.git(&["ls-files", "-u"]).stdout.is_empty());
}

fn pointer(data: &[u8]) -> String {
    format!(
        "version https://git-lfs.github.com/spec/v1\noid sha256:{:x}\nsize {}\n",
        Sha256::digest(data),
        data.len()
    )
}

#[cfg(unix)]
#[test]
fn lfs_transfers_use_local_storage_and_preserve_working_files() {
    use std::os::unix::fs::symlink;
    let Some(scratch) = Scratch::new() else {
        return;
    };
    let f = Fixture::unconfigured(&scratch, OsStr::new("transfer-source"), &[]);
    f.rpc(json!({"method":"lfs_install"}), true);
    fs::write(
        f.repo.join(".gitattributes"),
        "# preserve this comment\nordinary text=auto\n",
    )
    .unwrap();
    f.commit();
    f.rpc(json!({"method":"lfs_track","patterns":["*.bin"]}), true);
    let a = b"payload A\0\n";
    let b = b"payload B\n";
    let c = b"payload C on side\n";
    fs::write(f.repo.join("a.bin"), a).unwrap();
    fs::write(f.repo.join("b.bin"), b).unwrap();
    f.commit();
    f.git(&["switch", "-qc", "side"]);
    fs::write(f.repo.join("c.bin"), c).unwrap();
    f.commit();
    f.git(&["switch", "main"]);
    let remote = scratch.root.join("remote store.git");
    assert!(
        f.command("git")
            .args(["init", "--bare", "-q"])
            .arg(&remote)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        f.command("git")
            .arg("-C")
            .arg(&remote)
            .args(["symbolic-ref", "HEAD", "refs/heads/main"])
            .status()
            .unwrap()
            .success()
    );
    f.git(&["remote", "add", "origin", remote.to_str().unwrap()]);
    let remote_object = |body: &[u8]| {
        let oid = format!("{:x}", Sha256::digest(body));
        remote
            .join("lfs/objects")
            .join(&oid[..2])
            .join(&oid[2..4])
            .join(oid)
    };
    let result = f.cli(&["lfs", "push", "--dry-run"], true);
    let plan: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(
        plan["stdout"]
            .as_str()
            .unwrap()
            .contains(&format!("{:x}", Sha256::digest(a)))
    );
    assert!(
        plan["stdout"]
            .as_str()
            .unwrap()
            .contains(&format!("{:x}", Sha256::digest(b)))
    );
    assert!(!remote_object(a).exists());
    assert_eq!(
        f.rpc(
            json!({"method":"lfs_transfer","request":{"action":"push"}}),
            true
        )["code"],
        0
    );
    assert_eq!(fs::read(remote_object(a)).unwrap(), a);
    assert_eq!(fs::read(remote_object(b)).unwrap(), b);
    assert!(!remote_object(c).exists());
    assert_eq!(
        f.rpc(
            json!({"method":"lfs_transfer","request":{"action":"push","all":true}}),
            true
        )["code"],
        0
    );
    assert_eq!(fs::read(remote_object(c)).unwrap(), c);
    let hooks = scratch.root.join("no hooks");
    fs::create_dir(&hooks).unwrap();
    f.git(&[
        "-c",
        &format!("core.hooksPath={}", hooks.display()),
        "push",
        "origin",
        "main",
        "side",
    ]);
    let cloned = scratch.root.join("cloned");
    let output = f
        .command("git")
        .args(["clone", "-q"])
        .arg(&remote)
        .arg(&cloned)
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut x = Fixture {
        scratch: &scratch,
        repo: cloned.clone(),
        path: scratch.path.clone(),
        exec_path: None,
        literal_pathspecs: false,
    };
    x.rpc(json!({"method":"lfs_install"}), true);
    assert_eq!(
        fs::read(cloned.join("a.bin")).unwrap(),
        pointer(a).as_bytes()
    );
    assert!(!x.cache(a).exists());
    let index = x.git(&["ls-files", "--stage"]).stdout;
    let result = x.cli(&["lfs", "fetch", "--include", "a.bin"], true);
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stdout).unwrap()["code"],
        0
    );
    assert_eq!(fs::read(x.cache(a)).unwrap(), a);
    assert!(!x.cache(b).exists());
    assert_eq!(
        fs::read(cloned.join("a.bin")).unwrap(),
        pointer(a).as_bytes()
    );
    x.literal_pathspecs = true;
    let result = x.cli(&["lfs", "checkout", "a.bin"], true);
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stdout).unwrap()["code"],
        0
    );
    x.literal_pathspecs = false;
    assert_eq!(fs::read(cloned.join("a.bin")).unwrap(), a);
    assert_eq!(
        fs::read(cloned.join("b.bin")).unwrap(),
        pointer(b).as_bytes()
    );
    fs::write(cloned.join("a.bin"), b"user edits must survive").unwrap();
    assert_eq!(
        x.rpc(
            json!({"method":"lfs_transfer","request":{"action":"pull","include":"b.bin"}}),
            true
        )["code"],
        0
    );
    assert_eq!(
        fs::read(cloned.join("a.bin")).unwrap(),
        b"user edits must survive"
    );
    assert_eq!(fs::read(cloned.join("b.bin")).unwrap(), b);
    assert_eq!(x.git(&["ls-files", "--stage"]).stdout, index);
    fs::remove_file(x.cache(b)).unwrap();
    fs::write(cloned.join("b.bin"), pointer(b)).unwrap();
    x.rpc(
        json!({"method":"lfs_transfer","request":{"action":"fetch","dry_run":true}}),
        true,
    );
    assert!(!x.cache(b).exists());
    x.rpc(
        json!({"method":"lfs_transfer","request":{"action":"fetch","exclude":"a.bin"}}),
        true,
    );
    assert_eq!(fs::read(x.cache(b)).unwrap(), b);
    // Local transports may hard-link cache entries to the remote store.
    fs::remove_file(x.cache(b)).unwrap();
    fs::write(x.cache(b), b"corrupt").unwrap();
    x.rpc(
        json!({"method":"lfs_transfer","request":{"action":"fetch","refetch":true}}),
        true,
    );
    assert_eq!(fs::read(x.cache(b)).unwrap(), b);
    assert!(!x.cache(c).exists());
    x.rpc(
        json!({"method":"lfs_transfer","request":{"action":"fetch","all":true}}),
        true,
    );
    assert_eq!(fs::read(x.cache(c)).unwrap(), c);
    fs::write(cloned.join("a.bin"), pointer(a)).unwrap();
    fs::write(cloned.join("b.bin"), pointer(b)).unwrap();
    x.rpc(json!({"method":"lfs_checkout","patterns":["a.*"]}), true);
    assert_eq!(fs::read(cloned.join("a.bin")).unwrap(), a);
    assert_eq!(
        fs::read(cloned.join("b.bin")).unwrap(),
        pointer(b).as_bytes()
    );
    fs::remove_file(x.cache(b)).unwrap();
    x.rpc(json!({"method":"lfs_checkout"}), true);
    assert!(!x.cache(b).exists());
    assert_eq!(
        fs::read(cloned.join("b.bin")).unwrap(),
        pointer(b).as_bytes()
    );
    x.rpc(
        json!({"method":"lfs_transfer","request":{"action":"fetch"}}),
        true,
    );
    fs::remove_file(cloned.join("b.bin")).unwrap();
    let outside = scratch.root.join("outside-pointer");
    fs::write(&outside, pointer(b)).unwrap();
    symlink(&outside, cloned.join("b.bin")).unwrap();
    x.rpc(json!({"method":"lfs_checkout"}), true);
    assert_eq!(fs::read(outside).unwrap(), pointer(b).as_bytes());
    fs::remove_file(cloned.join("b.bin")).unwrap();
    fs::write(cloned.join("b.bin"), pointer(b)).unwrap();
    for request in [
        json!({"action":"fetch","all":true,"include":""}),
        json!({"action":"fetch","remote":"--bad"}),
        json!({"action":"fetch","remote":"missing"}),
        json!({"action":"fetch","refs":["not-a-commit"]}),
        json!({"action":"fetch","prune":true}),
    ] {
        x.rpc(json!({"method":"lfs_transfer","request":request}), false);
    }
}

#[cfg(unix)]
#[test]
fn lfs_http_authentication_is_redacted_and_cancellation_preserves_state() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use std::thread;
    use std::time::{Duration, Instant};

    struct HttpFixture {
        stop: Arc<AtomicBool>,
        block: Arc<AtomicBool>,
        contacts: Arc<AtomicUsize>,
        worker: Option<thread::JoinHandle<()>>,
        address: std::net::SocketAddr,
    }
    impl Drop for HttpFixture {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(worker) = self.worker.take() {
                worker.join().unwrap();
            }
        }
    }
    struct ChildGuard(Option<std::process::Child>);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            if let Some(child) = &mut self.0 {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
    const SECRET: &str = "synthetic-lfs-secret";
    let Some(scratch) = Scratch::new() else {
        return;
    };
    let f = Fixture::unconfigured(&scratch, OsStr::new("auth-source"), &[]);
    f.rpc(json!({"method":"lfs_install"}), true);
    f.rpc(json!({"method":"lfs_track","patterns":["*.bin"]}), true);
    let a = b"payload A\0\n";
    let b = b"payload B\n";
    fs::write(f.repo.join("a.bin"), a).unwrap();
    fs::write(f.repo.join("b.bin"), b).unwrap();
    f.commit();
    let remote = scratch.root.join("auth-remote.git");
    assert!(
        f.command("git")
            .args(["init", "--bare", "-q"])
            .arg(&remote)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        f.command("git")
            .arg("-C")
            .arg(&remote)
            .args(["symbolic-ref", "HEAD", "refs/heads/main"])
            .status()
            .unwrap()
            .success()
    );
    f.git(&["remote", "add", "origin", remote.to_str().unwrap()]);
    f.rpc(
        json!({"method":"lfs_transfer","request":{"action":"push"}}),
        true,
    );
    let hooks = scratch.root.join("auth-no-hooks");
    fs::create_dir(&hooks).unwrap();
    f.git(&[
        "-c",
        &format!("core.hooksPath={}", hooks.display()),
        "push",
        "origin",
        "main",
    ]);
    let cloned = scratch.root.join("auth-clone");
    let output = f
        .command("git")
        .args(["clone", "-q"])
        .arg(&remote)
        .arg(&cloned)
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let x = Fixture {
        scratch: &scratch,
        repo: cloned.clone(),
        path: scratch.path.clone(),
        exec_path: None,
        literal_pathspecs: false,
    };
    x.rpc(json!({"method":"lfs_install"}), true);
    x.rpc(
        json!({"method":"lfs_transfer","request":{"action":"fetch"}}),
        true,
    );
    x.rpc(json!({"method":"lfs_checkout","patterns":["a.bin"]}), true);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let block = Arc::new(AtomicBool::new(false));
    let contacts = Arc::new(AtomicUsize::new(0));
    let (worker_stop, worker_block, worker_contacts) =
        (stop.clone(), block.clone(), contacts.clone());
    let worker = thread::spawn(move || {
        while !worker_stop.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut request = [0; 8192];
                    if stream.read(&mut request).unwrap_or(0) == 0 {
                        continue;
                    }
                    worker_contacts.fetch_add(1, Ordering::AcqRel);
                    while worker_block.load(Ordering::Acquire)
                        && !worker_stop.load(Ordering::Acquire)
                    {
                        thread::sleep(Duration::from_millis(10));
                    }
                    let body = format!("401 Authentication failed {SECRET}");
                    let response = format!(
                        "HTTP/1.1 401 Unauthorized\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10))
                }
                Err(error) => panic!("HTTP fixture: {error}"),
            }
        }
    });
    let server = HttpFixture {
        stop,
        block,
        contacts,
        worker: Some(worker),
        address,
    };
    x.git(&[
        "config",
        "lfs.url",
        &format!("http://user:{SECRET}@{}/objects", server.address),
    ]);
    fs::remove_file(x.cache(b)).unwrap();
    let request = json!({"method":"lfs_transfer", "request":{"action":"fetch"}}).to_string();
    let failed = x.cli(&["repo", &request], false);
    let result: Value = serde_json::from_slice(&failed.stdout).unwrap();
    assert_ne!(result["code"], 0);
    assert_eq!(result["error_kind"], "authentication");
    assert!(!String::from_utf8_lossy(&failed.stdout).contains(SECRET));
    assert!(!String::from_utf8_lossy(&failed.stderr).contains(SECRET));
    let before = x.git(&["ls-files", "--stage"]).stdout;
    let working = fs::read(cloned.join("a.bin")).unwrap();
    let count = server.contacts.load(Ordering::Acquire);
    server.block.store(true, Ordering::Release);
    let mut child = ChildGuard(Some(
        x.command(env!("CARGO_BIN_EXE_desktop-rs"))
            .arg("-C")
            .arg(&cloned)
            .args([
                "repo",
                &json!({"method":"lfs_transfer","request":{"action":"pull"}}).to_string(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    let deadline = Instant::now() + Duration::from_secs(10);
    while server.contacts.load(Ordering::Acquire) == count {
        assert!(
            Instant::now() < deadline,
            "LFS pull did not reach local endpoint"
        );
        assert!(
            child.0.as_mut().unwrap().try_wait().unwrap().is_none(),
            "LFS pull exited before reaching endpoint"
        );
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        Command::new("kill")
            .args(["-INT", &child.0.as_ref().unwrap().id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.0.as_mut().unwrap().try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "cancelled LFS pull did not exit");
        thread::sleep(Duration::from_millis(20));
    }
    let output = child.0.take().unwrap().wait_with_output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(130),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!cloned.join(".git/index.lock").exists());
    assert_eq!(x.git(&["ls-files", "--stage"]).stdout, before);
    assert_eq!(fs::read(cloned.join("a.bin")).unwrap(), working);
}

#[cfg(unix)]
#[test]
fn lfs_checkout_preserves_staged_and_working_content_and_runs_hooks() {
    use std::os::unix::fs::PermissionsExt;
    let Some(scratch) = Scratch::new() else {
        return;
    };
    let mut f = Fixture::new(&scratch, "checkout-lfs", &[]);
    let file = f.repo.join("data.lfs");
    fs::write(&file, b"base payload\n").unwrap();
    f.commit();
    f.git(&["switch", "-qc", "other"]);
    fs::write(&file, b"target payload\n").unwrap();
    f.commit();
    f.git(&["switch", "-q", "main"]);
    fs::write(&file, b"index payload\n").unwrap();
    f.git(&["add", "--", "data.lfs"]);
    fs::write(&file, b"working payload\n").unwrap();
    let untracked = f.repo.join("new.lfs");
    fs::write(&untracked, b"untracked payload\n").unwrap();
    let hook = f.repo.join(".git/hooks/post-checkout");
    let original = fs::read_to_string(&hook).unwrap();
    let marker = scratch.root.join("hook-ran");
    let quoted = marker.to_string_lossy().replace('\'', "'\\''");
    let replacement = format!("#!/bin/sh\nprintf ran > '{quoted}'");
    assert!(original.contains("#!/bin/sh"));
    fs::write(&hook, original.replacen("#!/bin/sh", &replacement, 1)).unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
    f.literal_pathspecs = true;
    let preview = f.rpc(json!({"method":"checkout_preview", "branch":"other"}), true);
    let result = f.rpc(json!({"method":"checkout", "branch":"other", "strategy":"StashOnCurrentBranch", "basis":preview["basis"]}), true);
    let saved = result["saved_oid"].as_str().unwrap();
    assert_eq!(fs::read_to_string(&marker).unwrap(), "ran");
    assert_eq!(fs::read(&file).unwrap(), b"target payload\n");
    assert!(!untracked.exists());
    let preview = f.rpc(json!({"method":"checkout_preview", "branch":"main"}), true);
    f.rpc(json!({"method":"checkout", "branch":"main", "strategy":"MoveToNewBranch", "basis":preview["basis"]}), true);
    f.rpc(
        json!({"method":"stash_apply", "oid":saved, "restore_index":true, "pop":true}),
        true,
    );
    assert_eq!(fs::read(&file).unwrap(), b"working payload\n");
    assert_eq!(fs::read(&untracked).unwrap(), b"untracked payload\n");
    let index = String::from_utf8(f.git(&["show", ":data.lfs"]).stdout).unwrap();
    assert!(index.contains(&format!(
        "oid sha256:{:x}",
        Sha256::digest(b"index payload\n")
    )));
}

#[test]
fn lfs_stash_restores_index_and_working_payload_with_literal_pathspecs() {
    let Some(scratch) = Scratch::new() else {
        return;
    };
    let mut f = Fixture::new(&scratch, "stash-lfs", &[]);
    let file = f.repo.join("data.lfs");
    fs::write(&file, b"base payload\n").unwrap();
    f.commit();
    fs::write(&file, b"saved LFS payload\n").unwrap();
    f.git(&["add", "--", "data.lfs"]);
    fs::write(&file, b"working LFS payload\n").unwrap();
    f.literal_pathspecs = true;
    f.rpc(json!({"method":"stash_save", "message":"saved"}), true);
    let listing = f.rpc(json!({"method":"stash_list"}), true);
    let selected = &listing["entries"][0];
    f.rpc(json!({"method":"stash_apply", "oid":selected["oid"], "index":selected["index"], "basis":listing["basis"], "restore_index":true, "pop":true}), true);
    assert_eq!(fs::read(&file).unwrap(), b"working LFS payload\n");
    assert!(
        String::from_utf8(f.git(&["show", ":data.lfs"]).stdout)
            .unwrap()
            .contains("oid sha256:")
    );
}

#[test]
fn lfs_payload_formats_and_extension_pointers_are_conservative() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let Some(scratch) = Scratch::new() else {
        return;
    };
    let f = Fixture::new(&scratch, "formats", &[]);
    let target = f.repo.join("data.lfs");
    let png = STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Wl6SAAAAABJRU5ErkJggg==").unwrap();
    fs::write(&target, &png).unwrap();
    f.commit();
    let mut changed_png = png.clone();
    changed_png.extend_from_slice(b"after");
    fs::write(&target, changed_png).unwrap();
    let view = f.detail(json!("data.lfs"), false);
    assert_eq!(view["kind"], "image");
    assert_eq!(
        STANDARD
            .decode(view["before"]["image"]["data"].as_str().unwrap())
            .unwrap(),
        png
    );
    fs::write(&target, b"before\0binary").unwrap();
    f.commit();
    fs::write(&target, b"after\0binary").unwrap();
    assert_eq!(f.detail(json!("data.lfs"), false)["kind"], "binary");
    fs::write(&target, b"large\n".repeat(180000)).unwrap();
    f.commit();
    fs::write(&target, b"working small").unwrap();
    let large = f.detail(json!("data.lfs"), false);
    assert_eq!(large["kind"], "too_large");
    assert_eq!(large["before"]["lfs"]["state"], "too_large");
    assert_eq!(large["before"]["size"], 1_080_000);

    let encoded = b"encoded extension bytes";
    fs::write(&target, encoded).unwrap();
    f.git(&["add", "--", "data.lfs"]);
    let extended = pointer(encoded).replace(
        "oid sha256:",
        &format!("ext-0-example sha256:{}\noid sha256:", "a".repeat(64)),
    );
    let blob = scratch.root.join("extension-pointer");
    fs::write(&blob, extended).unwrap();
    let oid = String::from_utf8(
        f.git(&[
            "hash-object",
            "-w",
            "--no-filters",
            "--",
            blob.to_str().unwrap(),
        ])
        .stdout,
    )
    .unwrap();
    f.git(&[
        "update-index",
        "--cacheinfo",
        "100644",
        oid.trim(),
        "data.lfs",
    ]);
    let extension = f.detail(json!("data.lfs"), false);
    assert_eq!(extension["kind"], "lfs_unavailable");
    assert_eq!(extension["before"]["lfs"]["state"], "extension");
    let old = pointer(encoded)
        .trim_end_matches('\n')
        .replace("git-lfs.github", "hawser.github")
        .replace('\n', "\r\n")
        .replace("size ", "size +0");
    fs::write(&blob, old).unwrap();
    let oid = String::from_utf8(
        f.git(&[
            "hash-object",
            "-w",
            "--no-filters",
            "--",
            blob.to_str().unwrap(),
        ])
        .stdout,
    )
    .unwrap();
    f.git(&[
        "update-index",
        "--cacheinfo",
        "100644",
        oid.trim(),
        "data.lfs",
    ]);
    fs::write(&target, [encoded.as_slice(), b" edited"].concat()).unwrap();
    let view = f.detail(json!("data.lfs"), false);
    assert_eq!(view["before"]["oid"], oid.trim());
    assert_eq!(view["before"]["lfs"]["state"], "available");
    assert_eq!(view["can_select"], true);
    f.select_all(json!("data.lfs"), &view);
    assert_eq!(f.index("data.lfs"), fs::read(&target).unwrap());
    let marker = scratch.root.join("extension-must-not-run");
    f.git(&[
        "config",
        "lfs.extension.test.clean",
        &format!("touch {}", marker.display()),
    ]);
    fs::write(&target, [encoded.as_slice(), b" another edit"].concat()).unwrap();
    assert_eq!(f.detail(json!("data.lfs"), false)["can_select"], false);
    assert!(!marker.exists());
    f.git(&["config", "--remove-section", "lfs.extension.test"]);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut f = f;
        let fake = scratch.root.join("missing-lfs");
        fs::create_dir(&fake).unwrap();
        let tool = fake.join("git-lfs");
        fs::write(&tool, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o700)).unwrap();
        let mut paths = vec![fake.clone()];
        paths.extend(std::env::split_paths(&f.path));
        f.path = std::env::join_paths(paths).unwrap();
        f.exec_path = Some(fake);
        let unavailable = f.detail(json!("data.lfs"), false);
        assert_eq!(unavailable["kind"], "lfs_unavailable");
        assert_eq!(unavailable["before"]["lfs"]["state"], "unavailable");
    }
}
