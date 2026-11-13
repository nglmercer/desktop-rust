//! History, tracking, conflict resolution, and recovery through the real CLI.
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
};

struct Fixture {
    root: PathBuf,
    repo: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("desktop-history-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let f = Self {
            repo: root.join("repo"),
            root,
        };
        f.command("git")
            .args(["init", "-q", "-b", "main"])
            .arg(&f.repo)
            .status()
            .unwrap()
            .success()
            .then_some(())
            .expect("git init");
        f.git(&["config", "user.name", "Test Author"]);
        f.git(&["config", "user.email", "author@example.invalid"]);
        f.git(&["config", "commit.gpgsign", "false"]);
        f
    }

    fn command(&self, program: &str) -> Command {
        let mut cmd = Command::new(program);
        cmd.env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("GIT_CONFIG_SYSTEM")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.root.join("global"))
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_EDITOR", "true");
        cmd
    }

    fn git(&self, args: &[&str]) -> Output {
        let output = self
            .command("git")
            .arg("-C")
            .arg(&self.repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn git_text(&self, args: &[&str]) -> String {
        String::from_utf8(self.git(args).stdout)
            .unwrap()
            .trim()
            .to_owned()
    }

    fn request(&self, request: Value, success: bool) -> Value {
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
            .write_all(request.to_string().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.success(),
            success,
            "{request}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if !success {
            return Value::Null;
        }
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            value.get("code").and_then(Value::as_i64).unwrap_or(0),
            0,
            "{value}"
        );
        value
    }

    fn rpc(&self, value: Value) -> Value {
        self.request(value, true)
    }
    fn fails(&self, value: Value) {
        self.request(value, false);
    }
    fn write(&self, name: &str, data: impl AsRef<[u8]>) {
        fs::write(self.repo.join(name), data).unwrap();
    }
    fn commit(&self, message: &str) {
        self.git(&["add", "--all"]);
        self.git(&["commit", "-qm", message]);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn pinned_history_tracking_and_recovery() {
    let f = Fixture::new();
    assert_eq!(f.rpc(json!({"method":"history"}))["commits"], json!([]));
    f.write("file.txt", "initial\n");
    f.commit("Initial\n\nBody with delimiter \u{1e} and text");
    let initial = f.git_text(&["rev-parse", "HEAD"]);
    let root_files = f.rpc(json!({"method":"commit_files", "commit":initial}));
    assert!(root_files["parent"].is_null());
    assert_eq!(root_files["files"][0]["display_path"], "file.txt");
    let root_diff = f.rpc(json!({"method":"commit_diff", "path":"file.txt", "commit":initial}));
    assert_eq!(root_diff["can_select"], false);
    assert!(root_diff["before"]["mode"].is_null());
    for n in 0..5 {
        f.write("file.txt", format!("version {n}\n"));
        f.commit(&format!("Commit {n}"));
    }
    let first = f.rpc(json!({"method":"history", "limit":2}));
    assert_eq!(first["commits"].as_array().unwrap().len(), 2);
    assert_eq!(first["next_offset"], 2);
    let pinned = first["revision"].clone();
    f.write("file.txt", "later\n");
    f.commit("Later commit");
    let rest = f.rpc(json!({"method":"history", "revision":pinned, "offset":2, "limit":100}));
    let commits = rest["commits"].as_array().unwrap();
    assert_eq!(commits.len(), 4);
    assert_eq!(
        commits.last().unwrap()["message"],
        "Initial\n\nBody with delimiter \u{1e} and text\n"
    );
    assert_eq!(first["commits"][0]["author"]["name"], "Test Author");
    f.fails(json!({"method":"history", "limit":101}));
    f.fails(json!({"method":"history", "revision":"--help"}));
    f.git(&["mv", "file.txt", "renamed.txt"]);
    f.git(&["commit", "-qm", "Rename"]);
    let rename = f.git_text(&["rev-parse", "HEAD"]);
    let files = f.rpc(json!({"method":"commit_files", "commit":rename}));
    assert!(
        files["files"][0]["status"]
            .as_str()
            .unwrap()
            .starts_with('R')
    );
    assert_eq!(
        files["files"][0]["original_path"]["bytes"],
        json!(b"file.txt")
    );
    let diff = f.rpc(json!({"method":"commit_diff", "path":"renamed.txt", "commit":rename}));
    assert_eq!(diff["before"]["oid"], diff["after"]["oid"]);
    assert_eq!(diff["hunks"], json!([]));

    #[cfg(unix)]
    {
        use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
        let raw = b"raw-\xff";
        f.write("normal", "normal");
        fs::write(f.repo.join(OsStr::from_bytes(raw)), b"raw content\n").unwrap();
        f.commit("Raw filename");
        let commit = f.git_text(&["rev-parse", "HEAD"]);
        let files = f.rpc(json!({"method":"commit_files", "commit":commit}));
        assert!(
            files["files"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["path"]["bytes"] == json!(raw))
        );
        let diff = f.rpc(json!({"method":"commit_diff", "path":{"bytes":raw}, "commit":commit}));
        assert_eq!(diff["hunks"][0]["lines"][0]["text"], "raw content");
    }

    let remote = f.root.join("remote.git");
    assert!(
        f.command("git")
            .args(["init", "--bare", "-q"])
            .arg(&remote)
            .status()
            .unwrap()
            .success()
    );
    f.git(&["remote", "add", "origin", remote.to_str().unwrap()]);
    f.git(&["push", "-qu", "origin", "main"]);
    let snapshot = f.rpc(json!({"method":"snapshot"}));
    assert_eq!(snapshot["upstream"], "refs/remotes/origin/main");
    assert_eq!(snapshot["ahead"], 0);
    assert_eq!(snapshot["behind"], 0);
    f.write("ahead", "ahead");
    f.commit("Ahead");
    assert_eq!(f.rpc(json!({"method":"snapshot"}))["ahead"], 1);
    let branches = f.rpc(json!({"method":"branch_list"}));
    assert!(
        branches
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["current"] == true
                && b["name"] == "main"
                && b["upstream"] == "refs/remotes/origin/main")
    );
    f.fails(json!({"method":"rename_branch_preview", "name":"main", "new_name":"primary"}));
    f.git(&["switch", "-qc", "published-topic"]);
    f.git(&[
        "branch",
        "--set-upstream-to",
        "origin/main",
        "published-topic",
    ]);
    f.rpc(json!({"method":"rename_branch", "name":"published-topic", "new_name":"primary"}));
    f.rpc(json!({"method":"track_branch", "name":"primary", "upstream":"origin/main"}));
    assert_eq!(f.rpc(json!({"method":"snapshot"}))["branch"], "primary");
    f.rpc(json!({"method":"detach", "commit":initial}));
    assert!(f.rpc(json!({"method":"snapshot"}))["branch"].is_null());
    f.git(&["switch", "-q", "primary"]);

    f.git(&["switch", "-qc", "conflict"]);
    f.write("renamed.txt", "theirs\n");
    f.commit("Theirs");
    f.git(&["switch", "-q", "primary"]);
    f.write("renamed.txt", "ours\n");
    f.commit("Ours");
    f.fails(json!({"method":"merge", "branch":"conflict"}));
    assert_eq!(
        f.rpc(json!({"method":"snapshot"}))["operation"]["kind"],
        "merge"
    );
    let conflict = f.rpc(json!({"method":"conflict_detail", "path":"renamed.txt"}));
    assert_eq!(
        conflict["stages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["stage"].clone())
            .collect::<Vec<_>>(),
        vec![json!(1), json!(2), json!(3)]
    );
    f.write("renamed.txt", "manually edited\n");
    f.fails(json!({"method":"resolve_conflict", "path":"renamed.txt", "choice":"theirs", "basis":conflict["basis"]}));
    let conflict = f.rpc(json!({"method":"conflict_detail", "path":"renamed.txt"}));
    f.rpc(json!({"method":"resolve_conflict", "path":"renamed.txt", "choice":"theirs", "basis":conflict["basis"]}));
    assert_eq!(fs::read(f.repo.join("renamed.txt")).unwrap(), b"theirs\n");
    assert!(
        !f.rpc(json!({"method":"snapshot"}))["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["conflicted"] == true)
    );
    f.rpc(json!({"method":"recover", "kind":"merge", "action":"continue"}));
    assert!(f.rpc(json!({"method":"snapshot"}))["operation"].is_null());
    let merged = f.git_text(&["rev-parse", "HEAD"]);
    assert_eq!(
        f.rpc(json!({"method":"history", "limit":1}))["commits"][0]["parents"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(
        !f.rpc(json!({"method":"commit_files", "commit":merged, "parent":1}))["parent"].is_null()
    );
    f.rpc(json!({"method":"revert", "commit":merged, "mainline":1}));
    assert_eq!(fs::read(f.repo.join("renamed.txt")).unwrap(), b"ours\n");
    f.rpc(json!({"method":"cherry_pick", "commit":merged, "mainline":1}));
    assert_eq!(fs::read(f.repo.join("renamed.txt")).unwrap(), b"theirs\n");
    f.fails(json!({"method":"revert", "commit":merged, "mainline":0}));
    f.fails(json!({"method":"recover", "kind":"merge", "action":"abort"}));
    f.fails(json!({"method":"commit_files", "commit":initial, "parent":1}));

    f.git(&["switch", "-qc", "pick-base", &initial]);
    f.write("file.txt", "pick ours\n");
    f.commit("Pick base");
    f.git(&["switch", "-qc", "pick-source", &initial]);
    f.write("file.txt", "pick theirs\n");
    f.commit("Pick source");
    let pick = f.git_text(&["rev-parse", "HEAD"]);
    f.git(&["switch", "-q", "pick-base"]);
    f.fails(json!({"method":"cherry_pick", "commit":pick}));
    assert_eq!(
        f.rpc(json!({"method":"snapshot"}))["operation"]["kind"],
        "cherry_pick"
    );
    f.fails(json!({"method":"recover", "kind":"rebase", "action":"abort"}));
    f.rpc(json!({"method":"recover", "kind":"cherry_pick", "action":"abort"}));
    assert_eq!(fs::read(f.repo.join("file.txt")).unwrap(), b"pick ours\n");

    f.git(&["switch", "-qc", "delete-side"]);
    f.git(&["rm", "-q", "--", "file.txt"]);
    f.git(&["commit", "-qm", "Delete side"]);
    f.git(&["switch", "-q", "pick-base"]);
    f.write("file.txt", "modified ours\n");
    f.commit("Modify side");
    f.fails(json!({"method":"merge", "branch":"delete-side"}));
    let details = f.rpc(json!({"method":"conflict_detail", "path":"file.txt"}));
    assert_eq!(
        details["stages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["stage"].clone())
            .collect::<Vec<_>>(),
        vec![json!(1), json!(2)]
    );
    f.rpc(json!({"method":"resolve_conflict", "path":"file.txt", "choice":"theirs", "basis":details["basis"]}));
    assert!(!f.repo.join("file.txt").exists());
    f.rpc(json!({"method":"recover", "kind":"merge", "action":"continue"}));

    f.write("binary", b"\0base");
    f.commit("Binary base");
    f.git(&["switch", "-qc", "binary-side"]);
    f.write("binary", b"\0theirs");
    f.commit("Binary theirs");
    f.git(&["switch", "-q", "pick-base"]);
    f.write("binary", b"\0ours");
    f.commit("Binary ours");
    f.fails(json!({"method":"merge", "branch":"binary-side"}));
    let details = f.rpc(json!({"method":"conflict_detail", "path":"binary"}));
    assert!(
        details["stages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["text"].is_null())
    );
    f.rpc(json!({"method":"resolve_conflict", "path":"binary", "choice":"ours", "basis":details["basis"]}));
    assert_eq!(fs::read(f.repo.join("binary")).unwrap(), b"\0ours");
    f.rpc(json!({"method":"recover", "kind":"merge", "action":"continue"}));

    f.write("recover.txt", "A\n");
    f.commit("A");
    let base = f.git_text(&["rev-parse", "HEAD"]);
    f.git(&["switch", "-qc", "replay-source"]);
    f.write("recover.txt", "B\n");
    f.commit("B");
    let replay = f.git_text(&["rev-parse", "HEAD"]);
    let patch = f.git(&["format-patch", "-1", "--stdout"]).stdout;
    f.git(&["switch", "-qc", "replay-dest", &base]);
    f.write("recover.txt", "C\n");
    f.commit("C");
    f.fails(json!({"method":"rebase", "branch":"replay-source"}));
    assert_eq!(
        f.rpc(json!({"method":"snapshot"}))["operation"]["kind"],
        "rebase"
    );
    f.rpc(json!({"method":"recover", "kind":"rebase", "action":"abort"}));
    assert_eq!(fs::read(f.repo.join("recover.txt")).unwrap(), b"C\n");
    f.fails(json!({"method":"revert", "commit":replay}));
    assert_eq!(
        f.rpc(json!({"method":"snapshot"}))["operation"]["kind"],
        "revert"
    );
    f.rpc(json!({"method":"recover", "kind":"revert", "action":"abort"}));
    assert_eq!(fs::read(f.repo.join("recover.txt")).unwrap(), b"C\n");
    let mut am = f
        .command("git")
        .arg("-C")
        .arg(&f.repo)
        .arg("am")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    am.stdin.take().unwrap().write_all(&patch).unwrap();
    assert!(!am.wait_with_output().unwrap().status.success());
    assert_eq!(
        f.rpc(json!({"method":"snapshot"}))["operation"]["kind"],
        "am"
    );
    f.rpc(json!({"method":"recover", "kind":"am", "action":"abort"}));
    assert_eq!(fs::read(f.repo.join("recover.txt")).unwrap(), b"C\n");
    f.git(&[
        "commit",
        "--amend",
        "-qm",
        "C\n\nCo-authored-by: Helper <helper@example.invalid>",
    ]);
    assert_eq!(
        f.rpc(json!({"method":"history", "limit":1}))["commits"][0]["trailers"],
        json!([{"token":"Co-authored-by", "value":"Helper <helper@example.invalid>"}])
    );
    let output = f
        .command(env!("CARGO_BIN_EXE_desktop-rs"))
        .arg("-C")
        .arg(&f.repo)
        .args(["history", "--limit", "1"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let history: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(history["commits"][0]["summary"], "C");
}

#[test]
fn squash_abort_ignores_inherited_git_directory_overrides() {
    let f = Fixture::new();
    f.write("file", "base\n");
    f.commit("base");
    let head = f.git_text(&["rev-parse", "HEAD"]);
    let branch = f.git_text(&["symbolic-ref", "HEAD"]);
    let other = f.root.join("other");
    assert!(
        f.command("git")
            .arg("clone")
            .arg("-q")
            .arg(&f.repo)
            .arg(&other)
            .status()
            .unwrap()
            .success()
    );
    fs::write(f.repo.join("file"), "target changed\n").unwrap();
    fs::write(other.join("file"), "other changed\n").unwrap();
    fs::write(f.repo.join(".git/SQUASH_MSG"), "target\n").unwrap();
    fs::write(other.join(".git/SQUASH_MSG"), "other\n").unwrap();
    let mut redirected = f
        .command(env!("CARGO_BIN_EXE_desktop-rs"))
        .arg("-C")
        .arg(&f.repo)
        .args(["repo", "-"])
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.worktree")
        .env("GIT_CONFIG_VALUE_0", &other)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    redirected
        .stdin
        .take()
        .unwrap()
        .write_all(
            json!({"method":"renderer_abort_squash","expected_head":head,"expected_branch":branch})
                .to_string()
                .as_bytes(),
        )
        .unwrap();
    let redirected = redirected.wait_with_output().unwrap();
    if redirected.status.success() {
        let value: Value = serde_json::from_slice(&redirected.stdout).unwrap();
        assert_eq!(value["code"], 0, "{value}");
        assert_eq!(fs::read(f.repo.join("file")).unwrap(), b"base\n");
    } else {
        assert_eq!(fs::read(f.repo.join("file")).unwrap(), b"target changed\n");
    }
    assert_eq!(fs::read(other.join("file")).unwrap(), b"other changed\n");
    fs::write(f.repo.join("file"), "target changed\n").unwrap();
    fs::write(f.repo.join(".git/SQUASH_MSG"), "target\n").unwrap();
    let mut child = f
        .command(env!("CARGO_BIN_EXE_desktop-rs"))
        .arg("-C")
        .arg(&f.repo)
        .args(["repo", "-"])
        .env("GIT_DIR", other.join(".git"))
        .env("GIT_WORK_TREE", &other)
        .env("GIT_INDEX_FILE", other.join(".git/index"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            json!({"method":"renderer_abort_squash","expected_head":head,"expected_branch":branch})
                .to_string()
                .as_bytes(),
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["code"], 0, "{value}");
    assert_eq!(fs::read(f.repo.join("file")).unwrap(), b"base\n");
    assert_eq!(fs::read(other.join("file")).unwrap(), b"other changed\n");
    assert!(!f.repo.join(".git/SQUASH_MSG").exists());
    assert!(other.join(".git/SQUASH_MSG").exists());
}
