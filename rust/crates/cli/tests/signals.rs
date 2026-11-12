//! CLI signals clean up Git processes and preserve the selected repository state.
#![cfg(unix)]

use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread::sleep,
    time::{Duration, Instant},
};

struct Fixture {
    root: PathBuf,
    repo: PathBuf,
    pids: Vec<u32>,
}

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("desktop-cli-signals-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let repo = root.join("repo");
        let fixture = Self {
            root,
            repo,
            pids: Vec::new(),
        };
        fixture.git(
            &["init", "-q", "-b", "main", fixture.repo.to_str().unwrap()],
            false,
        );
        fixture.git(
            &[
                "-C",
                fixture.repo.to_str().unwrap(),
                "config",
                "user.name",
                "Signal Test",
            ],
            false,
        );
        fixture.git(
            &[
                "-C",
                fixture.repo.to_str().unwrap(),
                "config",
                "user.email",
                "signal@example.invalid",
            ],
            false,
        );
        fixture
    }

    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.root.join("gitconfig"))
            .env("GIT_TERMINAL_PROMPT", "0")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS");
        command
    }

    fn git(&self, args: &[&str], fail: bool) -> Output {
        let output = self.command("git").args(args).output().unwrap();
        assert_eq!(
            output.status.success(),
            !fail,
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn cli(&self) -> Command {
        let mut command = self.command(env!("CARGO_BIN_EXE_desktop-rs"));
        command.arg("--config").arg(self.root.join("registry.json"));
        command
    }

    fn repo_git(&self, args: &[&str]) -> String {
        let mut all = vec!["-C", self.repo.to_str().unwrap()];
        all.extend_from_slice(args);
        String::from_utf8(self.git(&all, false).stdout).unwrap()
    }

    fn rpc(&self, request: Value) -> Value {
        let output = self
            .cli()
            .arg("-C")
            .arg(&self.repo)
            .args(["repo", &request.to_string()])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{request}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let mut pids = self.pids.clone();
        if let Ok(marker) = fs::read_to_string(self.root.join("processes")) {
            pids.extend(
                marker
                    .split_whitespace()
                    .filter_map(|value| value.parse::<u32>().ok()),
            );
        }
        for pid in pids {
            if alive(pid) {
                let _ = signal(pid, "KILL");
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct ChildGuard(Option<Child>);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl ChildGuard {
    fn spawn(command: &mut Command) -> Self {
        Self(Some(
            command
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        ))
    }
    fn id(&self) -> u32 {
        self.0.as_ref().unwrap().id()
    }
    fn is_running(&mut self) -> bool {
        self.0.as_mut().unwrap().try_wait().unwrap().is_none()
    }
    fn output(&mut self, seconds: u64) -> Output {
        wait_for("CLI did not exit", seconds, || !self.is_running());
        self.0.take().unwrap().wait_with_output().unwrap()
    }
}

fn signal(pid: u32, name: &str) -> bool {
    Command::new("kill")
        .arg(format!("-{name}"))
        .arg(pid.to_string())
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

fn wait_for(label: &str, seconds: u64, mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !predicate() {
        assert!(Instant::now() < deadline, "{label}");
        sleep(Duration::from_millis(20));
    }
}

fn executable(path: &Path, source: &str) {
    fs::write(path, source).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn stash_shim(f: &Fixture) -> (PathBuf, std::ffi::OsString) {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let real = std::env::split_paths(&inherited)
        .map(|path| path.join("git"))
        .find(|path| path.is_file())
        .expect("git on PATH");
    let dir = f.root.join("git-wrapper");
    fs::create_dir(&dir).unwrap();
    executable(
        &dir.join("git"),
        r##"#!/bin/sh
stash=0
push=0
for arg in "$@"; do
  [ "$arg" = stash ] && stash=1
  [ "$arg" = push ] && push=1
done
"$REAL_GIT" "$@"
status=$?
if [ "$status" -eq 0 ] && [ "$stash" -eq 1 ] && [ "$push" -eq 1 ]; then
  case "$STASH_TEST_MODE" in
    source) "$REAL_GIT" -C "$STASH_REPO" symbolic-ref HEAD refs/heads/same-head || exit $? ;;
    target) "$REAL_GIT" -C "$STASH_REPO" update-ref refs/heads/other "$STASH_NEW" || exit $? ;;
    cancel) printf saved > "$STASH_STARTED"; sleep 60 ;;
  esac
fi
exit "$status"
"##,
    );
    let path = std::env::join_paths(std::iter::once(dir).chain(std::env::split_paths(&inherited)))
        .unwrap();
    (real, path)
}

fn switch_shim(f: &Fixture) -> (PathBuf, std::ffi::OsString) {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let real = std::env::split_paths(&inherited)
        .map(|path| path.join("git"))
        .find(|path| path.is_file())
        .expect("git on PATH");
    let dir = f.root.join("switch-wrapper");
    fs::create_dir(&dir).unwrap();
    executable(
        &dir.join("git"),
        r##"#!/bin/sh
switch=0
for arg in "$@"; do [ "$arg" = switch ] && switch=1; done
if [ "$switch" -eq 1 ]; then
  case "$SWITCH_TEST_MODE" in
    cancel) printf started > "$SWITCH_STARTED"; sleep 60 ;;
    before) exit 33 ;;
  esac
fi
"$REAL_GIT" "$@"
status=$?
if [ "$switch" -eq 1 ] && [ "$SWITCH_TEST_MODE" = after ] && [ "$status" -eq 0 ]; then exit 33; fi
exit "$status"
"##,
    );
    let path = std::env::join_paths(std::iter::once(dir).chain(std::env::split_paths(&inherited)))
        .unwrap();
    (real, path)
}

#[test]
fn stash_processes_serialize_and_waiting_lock_is_cancellable() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    fs::write(f.repo.join("file"), b"one\ntwo\n").unwrap();
    f.repo_git(&["add", "--", "file"]);
    f.repo_git(&["-c", "commit.gpgsign=false", "commit", "-qm", "base"]);
    for text in ["first", "second"] {
        fs::write(f.repo.join("file"), text).unwrap();
        f.rpc(json!({"method":"stash_save", "message":text}));
    }
    let list = f.rpc(json!({"method":"stash_list"}));
    let selected = &list["entries"][1];
    let request = json!({"method":"stash_drop", "oid":selected["oid"], "index":selected["index"], "basis":list["basis"]}).to_string();
    let spawn = || {
        let mut command = f.cli();
        command.arg("-C").arg(&f.repo).args(["repo", &request]);
        ChildGuard::spawn(&mut command)
    };
    let mut first = spawn();
    let mut second = spawn();
    let mut statuses = [
        first.output(15).status.code().unwrap(),
        second.output(15).status.code().unwrap(),
    ];
    statuses.sort();
    assert_eq!(statuses, [0, 1]);
    assert_eq!(
        f.rpc(json!({"method":"stash_list"}))["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    fs::write(f.repo.join("file"), b"keep waiting edit").unwrap();
    let path = f.repo.join(".git/desktop-stash.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .unwrap();
    lock.lock().unwrap();
    let mut command = f.cli();
    command
        .arg("-C")
        .arg(&f.repo)
        .args(["stash", "save", "blocked"]);
    let mut child = ChildGuard::spawn(&mut command);
    sleep(Duration::from_millis(150));
    assert!(child.is_running());
    assert!(signal(child.id(), "INT"));
    assert_eq!(child.output(6).status.code(), Some(130));
    assert_eq!(fs::read(f.repo.join("file")).unwrap(), b"keep waiting edit");
    drop(lock);
    assert_eq!(
        f.rpc(json!({"method":"stash_list"}))["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    fs::remove_file(&path).unwrap();
    let outside = f.root.join("outside-lock");
    fs::write(&outside, b"untouched").unwrap();
    symlink(&outside, &path).unwrap();
    let output = f
        .cli()
        .arg("-C")
        .arg(&f.repo)
        .args(["repo", &json!({"method":"stash_save"}).to_string()])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read(outside).unwrap(), b"untouched");
    assert_eq!(fs::read(f.repo.join("file")).unwrap(), b"keep waiting edit");
}

#[test]
fn stash_recovers_when_git_or_concurrent_writers_change_results() {
    for mode in [
        "push-error-after-save",
        "pop-concurrent-store",
        "move-drop-failure",
    ] {
        let f = Fixture::new();
        fs::write(f.repo.join("file"), b"one\ntwo\n").unwrap();
        f.repo_git(&["add", "--", "file"]);
        f.repo_git(&["-c", "commit.gpgsign=false", "commit", "-qm", "base"]);
        fs::write(f.repo.join("file"), b"saved data").unwrap();
        let saved = f.rpc(json!({"method":"stash_save", "message":"saved"}));
        let oid = saved["oid"].as_str().unwrap();
        if mode == "push-error-after-save" {
            fs::write(f.repo.join("file"), b"newly saved data").unwrap();
        }
        let external = if mode == "pop-concurrent-store" {
            f.repo_git(&[
                "commit-tree",
                &format!("{oid}^{{tree}}"),
                "-p",
                &format!("{oid}^1"),
                "-p",
                &format!("{oid}^2"),
                "-m",
                "concurrent saved stash",
            ])
            .trim()
            .to_owned()
        } else {
            oid.to_owned()
        };
        let list = f.rpc(json!({"method":"stash_list"}));
        let entry = &list["entries"][0];
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        let real = std::env::split_paths(&inherited)
            .map(|dir| dir.join("git"))
            .find(|path| path.is_file())
            .unwrap();
        let dir = f.root.join("stash-fault-git");
        fs::create_dir(&dir).unwrap();
        executable(
            &dir.join("git"),
            r##"#!/bin/sh
stash=0
action=''
for arg in "$@"; do
  [ "$arg" = stash ] && stash=1
  case "$arg" in push|apply|drop) action="$arg" ;; esac
done
if [ "$stash" -eq 1 ] && [ "$action" = drop ] && [ "$STASH_TEST_MODE" = move-drop-failure ]; then exit 42; fi
"$REAL_GIT" "$@"
status=$?
if [ "$stash" -eq 1 ] && [ "$status" -eq 0 ]; then
  if [ "$action" = apply ] && [ "$STASH_TEST_MODE" = pop-concurrent-store ]; then
    "$REAL_GIT" -C "$STASH_REPO" stash store -m 'concurrent store' "$STASH_EXTERNAL" || exit $?
  fi
  if [ "$action" = push ] && [ "$STASH_TEST_MODE" = push-error-after-save ]; then exit 1; fi
fi
exit "$status"
"##,
        );
        let path =
            std::env::join_paths(std::iter::once(dir).chain(std::env::split_paths(&inherited)))
                .unwrap();
        let mut request = match mode {
            "push-error-after-save" => {
                json!({"method":"stash_save","message":"failure after save"})
            }
            "pop-concurrent-store" => {
                json!({"method":"stash_apply","oid":entry["oid"],"index":entry["index"],"basis":list["basis"],"pop":true})
            }
            _ => {
                json!({"method":"stash_move","oid":entry["oid"],"index":entry["index"],"basis":list["basis"],"branch":"new-branch"})
            }
        };
        let output = f
            .cli()
            .arg("-C")
            .arg(&f.repo)
            .args(["repo", &request.to_string()])
            .env("PATH", path)
            .env("REAL_GIT", real)
            .env("STASH_REPO", &f.repo)
            .env("STASH_TEST_MODE", mode)
            .env("STASH_EXTERNAL", external)
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            mode != "push-error-after-save",
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        request = serde_json::from_slice(&output.stdout).unwrap();
        let current = f.rpc(json!({"method":"stash_list"}));
        let entries = current["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2, "{mode}");
        match mode {
            "push-error-after-save" => {
                assert_eq!(request["code"], 1);
                assert_eq!(request["created"], true);
                let created = request["oid"].as_str().unwrap();
                assert_eq!(
                    f.repo_git(&["show", &format!("{created}:file")]),
                    "newly saved data"
                );
            }
            "pop-concurrent-store" => {
                assert_eq!(request["retained"], true);
                assert!(
                    request["warning"]
                        .as_str()
                        .unwrap()
                        .contains("list changed")
                );
                assert_eq!(fs::read(f.repo.join("file")).unwrap(), b"saved data");
            }
            _ => {
                assert_eq!(request["original_retained"], true);
                assert!(entries.iter().any(|entry| entry["oid"] == oid));
            }
        }
    }
}

#[test]
fn interrupted_checkout_keeps_receipts_and_recovers_unrecorded_stashes() {
    for mode in ["before", "after", "cancel"] {
        let f = Fixture::new();
        fs::write(f.repo.join("file"), b"base\n").unwrap();
        f.repo_git(&["add", "--", "file"]);
        f.repo_git(&["-c", "commit.gpgsign=false", "commit", "-qm", "base"]);
        f.repo_git(&["switch", "-qc", "other"]);
        fs::write(f.repo.join("file"), b"target\n").unwrap();
        f.repo_git(&["add", "--", "file"]);
        f.repo_git(&["-c", "commit.gpgsign=false", "commit", "-qm", "target"]);
        f.repo_git(&["switch", "-q", "main"]);
        fs::write(f.repo.join("file"), b"recover saved work").unwrap();
        let plan = f.rpc(json!({"method":"checkout_preview", "branch":"other"}));
        let (real, path) = switch_shim(&f);
        let marker = f.root.join("switch-started");
        let mut command = f.cli();
        command
            .arg("-C")
            .arg(&f.repo)
            .args([
                "checkout",
                "other",
                "--strategy",
                "leave",
                "--basis",
                plan["basis"].as_str().unwrap(),
            ])
            .env("PATH", path)
            .env("REAL_GIT", real)
            .env("SWITCH_TEST_MODE", mode)
            .env("SWITCH_STARTED", &marker);
        let mut child = ChildGuard::spawn(&mut command);
        if mode == "cancel" {
            wait_for("Git switch did not start", 10, || {
                marker.exists() || !child.is_running()
            });
            assert!(marker.exists(), "checkout exited before Git switch");
            assert!(signal(child.id(), "INT"));
        }
        let output = child.output(10);
        assert!(
            !output.status.success(),
            "{mode}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        if mode == "cancel" {
            assert_eq!(output.status.code(), Some(130));
        }
        let history = f.rpc(json!({"method":"checkout_history"}));
        let item = &history[0];
        assert_eq!(item["receipt"]["phase"], "switching");
        assert!(!item["receipt"]["error"].is_null());
        let oid = item["saved_stashes"][0]["oid"].as_str().unwrap();
        assert_eq!(
            f.repo_git(&["show", &format!("{oid}:file")]),
            "recover saved work"
        );
        assert_eq!(
            f.repo_git(&["symbolic-ref", "--short", "HEAD"]).trim(),
            if mode == "after" { "other" } else { "main" }
        );
        assert!(!f.repo.join(".git/index.lock").exists());
        if mode == "cancel" {
            let id = item["receipt"]["id"].as_str().unwrap();
            let manifest = f
                .repo
                .join(".git/desktop-checkout")
                .join(format!("{id}.json"));
            let mut data: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
            data["saved_oid"] = Value::Null;
            data["phase"] = json!("stashing");
            data["error"] = Value::Null;
            fs::write(&manifest, serde_json::to_vec(&data).unwrap()).unwrap();
            assert_eq!(
                f.rpc(json!({"method":"checkout_history"}))[0]["saved_stashes"][0]["oid"],
                oid
            );
        }
    }
}

#[test]
fn checkout_stash_races_keep_the_previous_saved_work() {
    for mode in ["source", "target"] {
        let f = Fixture::new();
        fs::write(f.repo.join("file"), b"base\n").unwrap();
        f.repo_git(&["add", "--", "file"]);
        f.repo_git(&["-c", "commit.gpgsign=false", "commit", "-qm", "base"]);
        f.repo_git(&["branch", "other"]);
        f.repo_git(&["branch", "same-head"]);
        fs::write(f.repo.join("file"), b"previous work").unwrap();
        let previous = f.rpc(json!({"method":"stash_save", "desktop":true}))["oid"]
            .as_str()
            .unwrap()
            .to_owned();
        fs::write(f.repo.join("file"), b"new saved work").unwrap();
        let plan = f.rpc(json!({"method":"checkout_preview", "branch":"other"}));
        let new = f.repo_git(&["commit-tree", "HEAD^{tree}", "-m", "new target"]);
        let (real, path) = stash_shim(&f);
        let request = json!({"method":"checkout", "branch":"other", "strategy":"StashOnCurrentBranch", "basis":plan["basis"], "replace_stash":true});
        let output = f
            .cli()
            .arg("-C")
            .arg(&f.repo)
            .args(["repo", &request.to_string()])
            .env("PATH", path)
            .env("REAL_GIT", real)
            .env("STASH_REPO", &f.repo)
            .env("STASH_TEST_MODE", mode)
            .env("STASH_NEW", new.trim())
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "{mode}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let list = f.rpc(json!({"method":"stash_list"}));
        let entries = list["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|item| item["oid"] == previous));
        let history = f.rpc(json!({"method":"checkout_history"}));
        assert_eq!(history[0]["receipt"]["phase"], "stashed");
        assert!(!history[0]["saved_stashes"].as_array().unwrap().is_empty());
    }
}

#[test]
fn interrupted_stash_creation_reconciles_saved_data() {
    let f = Fixture::new();
    fs::write(f.repo.join("file"), b"base\n").unwrap();
    f.repo_git(&["add", "--", "file"]);
    f.repo_git(&["-c", "commit.gpgsign=false", "commit", "-qm", "base"]);
    f.repo_git(&["branch", "other"]);
    fs::write(f.repo.join("file"), b"saved before cancellation").unwrap();
    let plan = f.rpc(json!({"method":"checkout_preview", "branch":"other"}));
    let (real, path) = stash_shim(&f);
    let marker = f.root.join("stash-started");
    let mut command = f.cli();
    command
        .arg("-C")
        .arg(&f.repo)
        .args([
            "checkout",
            "other",
            "--strategy",
            "leave",
            "--basis",
            plan["basis"].as_str().unwrap(),
        ])
        .env("PATH", path)
        .env("REAL_GIT", real)
        .env("STASH_TEST_MODE", "cancel")
        .env("STASH_STARTED", &marker);
    let mut child = ChildGuard::spawn(&mut command);
    wait_for("stash was not saved", 10, || {
        marker.exists() || !child.is_running()
    });
    assert!(marker.exists(), "checkout exited before saving the stash");
    assert!(signal(child.id(), "INT"));
    let result = child.output(10);
    assert_eq!(
        result.status.code(),
        Some(130),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let history = f.rpc(json!({"method":"checkout_history"}));
    let item = &history[0];
    assert_eq!(item["receipt"]["phase"], "stashing");
    assert!(item["receipt"]["saved_oid"].is_null());
    let oid = item["saved_stashes"][0]["oid"].as_str().unwrap();
    assert_eq!(
        f.repo_git(&["show", &format!("{oid}:file")]),
        "saved before cancellation"
    );
}

#[test]
fn signals_preserve_git_and_metadata_and_leave_passthrough_untouched() {
    let mut f = Fixture::new();
    fs::write(f.repo.join("file.txt"), b"initial\n").unwrap();
    f.repo_git(&["add", "--", "file.txt"]);
    f.repo_git(&["-c", "commit.gpgsign=false", "commit", "-qm", "initial"]);
    let original = f.repo_git(&["rev-parse", "HEAD"]);
    fs::write(f.repo.join("file.txt"), b"staged\n").unwrap();
    f.repo_git(&["add", "--", "file.txt"]);
    let marker = f.root.join("processes");
    let ready = f.root.join("child-ready");
    let hook = f.repo.join(".git/hooks/pre-commit");
    executable(
        &hook,
        "#!/bin/sh\n(trap '' INT; printf ready > \"$CHILD_READY\"; exec sleep 60) &\nchild=$!\nwhile [ ! -f \"$CHILD_READY\" ]; do sleep 0.02; done\nprintf '%s %s' \"$$\" \"$child\" > \"$SIGNAL_MARKER\"\nwait \"$child\"\n",
    );
    for (sig, expected) in [("INT", 130), ("TERM", 143)] {
        let mut command = f.cli();
        command
            .arg("-C")
            .arg(&f.repo)
            .args(["commit", "must not commit"])
            .env("SIGNAL_MARKER", &marker)
            .env("CHILD_READY", &ready);
        let mut child = ChildGuard::spawn(&mut command);
        wait_for("commit hook did not start", 10, || {
            fs::read_to_string(&marker).is_ok_and(|s| s.split_whitespace().count() == 2)
        });
        let pids: Vec<u32> = fs::read_to_string(&marker)
            .unwrap()
            .split_whitespace()
            .map(|p| p.parse().unwrap())
            .collect();
        f.pids.extend(&pids);
        assert!(signal(child.id(), sig));
        let output = child.output(10);
        assert_eq!(
            output.status.code(),
            Some(expected),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        wait_for("signal left Git descendants running", 10, || {
            pids.iter().all(|pid| !alive(*pid))
        });
        f.pids.clear();
        assert!(!f.repo.join(".git/index.lock").exists());
        assert_eq!(f.repo_git(&["rev-parse", "HEAD"]), original);
        assert_eq!(f.repo_git(&["show", ":file.txt"]), "staged\n");
        fs::remove_file(&marker).unwrap();
        fs::remove_file(&ready).unwrap();
    }

    let output = f
        .cli()
        .args(["preferences", "set", r#"{"theme":"light"}"#])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let preferences = f.root.join("registry.preferences.json");
    let original_preferences = fs::read(&preferences).unwrap();
    let lock: File = OpenOptions::new()
        .read(true)
        .write(true)
        .open(preferences.with_extension("lock"))
        .unwrap();
    lock.lock().unwrap();
    let mut command = f.cli();
    command.args(["preferences", "set", r#"{"theme":"dark"}"#]);
    let mut child = ChildGuard::spawn(&mut command);
    sleep(Duration::from_millis(150));
    assert!(child.is_running());
    assert!(signal(child.id(), "INT"));
    let output = child.output(6);
    assert_eq!(
        output.status.code(),
        Some(130),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(preferences).unwrap(), original_preferences);
    drop(lock);

    let tools = f.root.join("bin");
    fs::create_dir(&tools).unwrap();
    executable(
        &tools.join("gh"),
        "#!/bin/sh\ntrap 'exit 42' INT\nprintf ready > \"$SIGNAL_MARKER\"\nwhile :; do sleep 0.05; done\n",
    );
    let path = std::env::join_paths(std::iter::once(tools).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    let mut command = f.cli();
    command
        .args(["gh", "custom-command"])
        .env("PATH", path)
        .env("SIGNAL_MARKER", &marker);
    let mut child = ChildGuard::spawn(&mut command);
    wait_for("passthrough did not start", 10, || marker.exists());
    assert!(signal(child.id(), "INT"));
    assert_eq!(child.output(6).status.code(), Some(42));
}
