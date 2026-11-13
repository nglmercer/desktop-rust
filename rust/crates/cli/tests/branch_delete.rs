//! Reviewed branch deletion, recovery and native Git coordination.
#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

struct Suite {
    root: PathBuf,
    env: HashMap<OsString, OsString>,
}
impl Suite {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("desktop-branch-delete-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        Self {
            root,
            env: HashMap::new(),
        }
    }
    fn command(&self, executable: &str) -> Command {
        let mut command = Command::new(executable);
        command
            .env("HOME", &self.root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.root.join("global"))
            .env("GIT_TERMINAL_PROMPT", "0")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .envs(&self.env);
        command
    }
    fn run<I, S>(&self, executable: &str, args: I, input: Option<&[u8]>, fail: bool) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = self.command(executable);
        command
            .args(args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        if let Some(input) = input {
            child.stdin.take().unwrap().write_all(input).unwrap();
        }
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.success(),
            !fail,
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
    fn git_result(&self, repo: &Path, args: &[&str], fail: bool) -> Output {
        let mut all: Vec<OsString> = vec!["-C".into(), repo.as_os_str().into()];
        all.extend(args.iter().map(OsString::from));
        self.run("git", all, None, fail)
    }
    fn git(&self, repo: &Path, args: &[&str]) -> String {
        String::from_utf8(self.git_result(repo, args, false).stdout)
            .unwrap()
            .trim()
            .to_owned()
    }
    fn seed(&self, name: &str, options: &[&str]) -> PathBuf {
        let repo = self.root.join(name);
        let mut all: Vec<OsString> = ["init", "-q", "-b", "main"]
            .into_iter()
            .map(Into::into)
            .collect();
        all.extend(options.iter().map(OsString::from));
        all.push(repo.as_os_str().into());
        self.run("git", all, None, false);
        self.git(&repo, &["config", "user.name", "Branch deletion test"]);
        self.git(&repo, &["config", "user.email", "delete@example.invalid"]);
        self.git(&repo, &["config", "commit.gpgsign", "false"]);
        fs::write(
            repo.join("file"),
            (0..30).map(|i| format!("line {i}\n")).collect::<String>(),
        )
        .unwrap();
        self.commit(&repo);
        repo
    }
    fn commit(&self, repo: &Path) {
        self.git(repo, &["add", "--all"]);
        self.git(repo, &["commit", "-qm", "checkpoint"]);
    }
    fn rpc_raw(&self, repo: &Path, method: &str, mut params: Value, fail: bool) -> Output {
        params["method"] = json!(method);
        self.run(
            env!("CARGO_BIN_EXE_desktop-rs"),
            [
                OsString::from("-C"),
                repo.as_os_str().into(),
                "repo".into(),
                "-".into(),
            ],
            Some(params.to_string().as_bytes()),
            fail,
        )
    }
    fn rpc(&self, repo: &Path, method: &str, params: Value) -> Value {
        serde_json::from_slice(&self.rpc_raw(repo, method, params, false).stdout).unwrap()
    }
    fn delete(&self, repo: &Path, name: &str, mut params: Value, fail: bool) -> Output {
        let mut review = json!({"name":name});
        if let Some(target) = params.get("to_checkout") {
            review["to_checkout"] = target.clone();
        }
        let plan = self.rpc(repo, "delete_branch_preview", review);
        params["name"] = json!(name);
        params["basis"] = plan["basis"].clone();
        self.rpc_raw(repo, "delete_branch", params, fail)
    }
    fn result(&self, repo: &Path, name: &str, params: Value) -> Value {
        serde_json::from_slice(&self.delete(repo, name, params, false).stdout).unwrap()
    }
    fn exists(&self, repo: &Path, name: &str) -> bool {
        self.command("git")
            .args([
                "-C",
                repo.to_str().unwrap(),
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{name}"),
            ])
            .status()
            .unwrap()
            .success()
    }
    fn entries(&self, repo: &Path) -> Value {
        self.rpc(repo, "deleted_branches", json!({}))
    }
}
impl Drop for Suite {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
const LEAVE: &str = "StashOnCurrentBranch";
const BRING: &str = "MoveToNewBranch";

#[test]
fn ref_only_deletion_cannot_switch_or_delete_current_branch() {
    let s = Suite::new();
    let repo = s.seed("ref-only", &[]);
    s.git(&repo, &["switch", "-qc", "victim"]);
    let result = s.delete(&repo, "victim", json!({"force":true,"ref_only":true}), true);
    assert!(String::from_utf8_lossy(&result.stderr).contains("Ref-only branch deletion"));
    assert_eq!(s.git(&repo, &["branch", "--show-current"]), "victim");
    assert!(s.exists(&repo, "victim"));
    assert_eq!(s.entries(&repo), json!([]));
}

#[test]
fn reviewed_deletion_recovery_and_dirty_checkout() {
    let s = Suite::new();
    let repo = s.seed("normal", &[]);
    s.git(&repo, &["branch", "victim"]);
    assert_eq!(s.entries(&repo), json!([]));
    assert!(!repo.join(".git/desktop-branch-deletions").exists());
    s.git(
        &repo,
        &["config", "branch.victim.description", "saved\ntext"],
    );
    s.git(
        &repo,
        &["config", "--add", "branch.victim.merge", "refs/heads/main"],
    );
    s.git(
        &repo,
        &["config", "--add", "branch.victim.merge", "refs/heads/other"],
    );
    s.git(&repo, &["branch", "victim.child"]);
    s.git(
        &repo,
        &[
            "config",
            "branch.victim.child.description",
            "keep other branch",
        ],
    );
    fs::write(repo.join("file"), "index").unwrap();
    s.git(&repo, &["add", "--", "file"]);
    fs::write(repo.join("file"), "working").unwrap();
    fs::write(repo.join("loose"), "loose").unwrap();
    let index = fs::read(repo.join(".git/index")).unwrap();
    let status = s
        .git_result(&repo, &["status", "--porcelain=v2", "-z"], false)
        .stdout;
    let tip = s.git(&repo, &["rev-parse", "victim"]);
    let plan = s.rpc(
        &repo,
        "delete_branch_preview",
        json!({"name":"refs/heads/victim"}),
    );
    assert_eq!(plan["force_required"], false);
    assert!(plan["checkout"].is_null());
    assert!(!repo.join(".git/desktop-branch-deletions").exists());
    let result = s.rpc(
        &repo,
        "delete_branch",
        json!({"name":"refs/heads/victim","basis":plan["basis"]}),
    );
    assert!(!s.exists(&repo, "victim"));
    let record = &s.entries(&repo)[0];
    assert_eq!(record["phase"], "complete");
    assert_eq!(record["backup_oid"], tip);
    assert!(record["current_oid"].is_null());
    assert!(record.get("config").is_none());
    assert_eq!(fs::read(repo.join(".git/index")).unwrap(), index);
    assert_eq!(
        s.git_result(&repo, &["status", "--porcelain=v2", "-z"], false)
            .stdout,
        status
    );
    assert_eq!(
        s.git(&repo, &["config", "branch.victim.child.description"]),
        "keep other branch"
    );
    s.git(
        &repo,
        &[
            "symbolic-ref",
            "refs/heads/dangling-restore",
            "refs/heads/leave-absent",
        ],
    );
    s.rpc_raw(
        &repo,
        "restore_deleted_branch",
        json!({"receipt_id":result["receipt_id"],"name":"dangling-restore"}),
        true,
    );
    assert!(!s.exists(&repo, "leave-absent"));
    assert_eq!(
        s.git(&repo, &["symbolic-ref", "refs/heads/dangling-restore"]),
        "refs/heads/leave-absent"
    );
    s.rpc(
        &repo,
        "restore_deleted_branch",
        json!({"receipt_id":result["receipt_id"]}),
    );
    assert!(s.exists(&repo, "victim"));
    assert_eq!(
        s.git_result(&repo, &["config", "branch.victim.description"], false)
            .stdout,
        b"saved\ntext\n"
    );
    assert_eq!(
        s.git_result(
            &repo,
            &["config", "--get-all", "branch.victim.merge"],
            false
        )
        .stdout,
        b"refs/heads/main\nrefs/heads/other\n"
    );
    assert_eq!(
        s.git_result(&repo, &["status", "--porcelain=v2", "-z"], false)
            .stdout,
        status
    );
    assert_eq!(s.entries(&repo)[0]["phase"], "restored");
    s.rpc(
        &repo,
        "forget_deleted_branch",
        json!({"receipt_id":result["receipt_id"]}),
    );
    assert_eq!(s.entries(&repo), json!([]));
    assert!(s.exists(&repo, "victim"));
    let repo = s.seed("unmerged", &[]);
    s.git(&repo, &["switch", "-qc", "victim"]);
    fs::write(repo.join("unique"), "unmerged commit").unwrap();
    s.commit(&repo);
    let tip = s.git(&repo, &["rev-parse", "HEAD"]);
    s.git(&repo, &["switch", "-q", "main"]);
    assert_eq!(
        s.rpc(&repo, "delete_branch_preview", json!({"name":"victim"}))["force_required"],
        true
    );
    let refused = s.delete(&repo, "victim", json!({}), true);
    assert_eq!(
        serde_json::from_slice::<Value>(&refused.stdout).unwrap()["requires_force"],
        true
    );
    assert!(s.exists(&repo, "victim"));
    assert!(!repo.join(".git/desktop-branch-deletions").exists());
    let result = s.result(&repo, "victim", json!({"force":true}));
    s.git(&repo, &["reflog", "expire", "--expire=now", "--all"]);
    s.git(&repo, &["gc", "--prune=now"]);
    assert_eq!(
        s.git_result(
            &repo,
            &[
                "show",
                &format!("{}:unique", result["backup_ref"].as_str().unwrap())
            ],
            false
        )
        .stdout,
        b"unmerged commit"
    );
    s.git(&repo, &["branch", "victim"]);
    s.rpc_raw(
        &repo,
        "restore_deleted_branch",
        json!({"receipt_id":result["receipt_id"]}),
        true,
    );
    s.rpc(
        &repo,
        "restore_deleted_branch",
        json!({"receipt_id":result["receipt_id"],"name":"restored-tip"}),
    );
    assert_eq!(s.git(&repo, &["rev-parse", "restored-tip"]), tip);
    let repo = s.seed("current", &[]);
    s.git(&repo, &["switch", "-qc", "victim"]);
    fs::write(repo.join("unique"), "commit").unwrap();
    s.commit(&repo);
    fs::write(repo.join("file"), "previous").unwrap();
    let old = s.rpc(&repo, "stash_save", json!({"desktop":true}))["oid"].clone();
    fs::write(repo.join("file"), "index").unwrap();
    s.git(&repo, &["add", "--", "file"]);
    fs::write(repo.join("file"), "working").unwrap();
    fs::write(repo.join("loose"), b"loose\0").unwrap();
    assert_eq!(
        s.rpc(&repo, "delete_branch_preview", json!({"name":"victim"}))["checkout"]["target"]["branch"],
        "main"
    );
    for options in [
        json!({"force":true}),
        json!({"force":true,"strategy":LEAVE}),
    ] {
        let output = s.delete(&repo, "victim", options, true);
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap()["requires_confirmation"],
            true
        );
        assert!(s.exists(&repo, "victim"));
        assert!(!repo.join(".git/desktop-branch-deletions").exists());
    }
    let result = s.result(
        &repo,
        "victim",
        json!({"force":true,"strategy":LEAVE,"replace_stash":true}),
    );
    assert_eq!(s.rpc(&repo, "snapshot", json!({}))["branch"], "main");
    assert!(!s.exists(&repo, "victim"));
    let stashes = s.rpc(&repo, "stash_list", json!({}));
    let saved = stashes["entries"].as_array().unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0]["branch"], "victim");
    assert_ne!(saved[0]["oid"], old);
    for (revision, expected) in [
        (
            format!("{}:file", saved[0]["oid"].as_str().unwrap()),
            b"working".as_slice(),
        ),
        (
            format!("{}:file", saved[0]["parents"][1].as_str().unwrap()),
            b"index",
        ),
        (
            format!("{}:loose", saved[0]["parents"][2].as_str().unwrap()),
            b"loose\0",
        ),
    ] {
        assert_eq!(
            s.git_result(&repo, &["show", &revision], false).stdout,
            expected
        );
    }
    let record = s.entries(&repo)[0].clone();
    assert!(
        s.rpc(&repo, "checkout_history", json!({}))
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["receipt"]["id"] == record["checkout_receipt_id"])
    );
    s.rpc(
        &repo,
        "restore_deleted_branch",
        json!({"receipt_id":result["receipt_id"]}),
    );
    assert!(s.exists(&repo, "victim"));
    assert_eq!(s.rpc(&repo, "snapshot", json!({}))["branch"], "main");
    let repo = s.seed("bring", &[]);
    s.git(&repo, &["switch", "-qc", "victim"]);
    fs::write(repo.join("file"), "index").unwrap();
    s.git(&repo, &["add", "--", "file"]);
    fs::write(repo.join("file"), "working").unwrap();
    s.result(&repo, "victim", json!({"strategy":BRING}));
    assert_eq!(s.rpc(&repo, "snapshot", json!({}))["branch"], "main");
    assert_eq!(
        s.git_result(&repo, &["show", ":file"], false).stdout,
        b"index"
    );
    assert_eq!(fs::read(repo.join("file")).unwrap(), b"working");
}

#[test]
fn stale_reviews_native_backends_and_recovery_records() {
    let s = Suite::new();
    for kind in ["ref", "config", "index", "bytes", "stash", "target"] {
        let repo = s.seed(&format!("stale-{kind}"), &[]);
        s.git(&repo, &["branch", "victim"]);
        s.git(&repo, &["branch", "other"]);
        let loose = repo.join("loose");
        fs::write(&loose, "one").unwrap();
        let plan = s.rpc(
            &repo,
            "delete_branch_preview",
            json!({"name":"victim","to_checkout":"other"}),
        );
        match kind {
            "ref" => {
                let oid = s.git(&repo, &["commit-tree", "HEAD^{tree}", "-m", "different"]);
                s.git(&repo, &["update-ref", "refs/heads/victim", &oid]);
            }
            "config" => {
                s.git(&repo, &["config", "branch.victim.description", "changed"]);
            }
            "index" => {
                s.git(&repo, &["add", "--", "loose"]);
            }
            "bytes" => {
                let metadata = fs::metadata(&loose).unwrap();
                fs::write(&loose, "two").unwrap();
                let times = fs::FileTimes::new()
                    .set_accessed(metadata.accessed().unwrap())
                    .set_modified(metadata.modified().unwrap());
                fs::File::options()
                    .write(true)
                    .open(&loose)
                    .unwrap()
                    .set_times(times)
                    .unwrap();
            }
            "stash" => {
                fs::write(repo.join("file"), "stashed").unwrap();
                s.rpc(&repo, "stash_save", json!({"message":"new stash"}));
            }
            "target" => {
                let oid = s.git(&repo, &["commit-tree", "HEAD^{tree}", "-m", "new target"]);
                s.git(&repo, &["update-ref", "refs/heads/other", &oid]);
            }
            _ => unreachable!(),
        }
        s.rpc_raw(&repo, "delete_branch", json!({"name":"victim","to_checkout":"other","basis":plan["basis"],"force":true,"strategy":BRING}), true);
        assert!(s.exists(&repo, "victim"));
        assert!(!repo.join(".git/desktop-branch-deletions").exists());
    }
    let repo = s.seed("held", &[]);
    let held = s.root.join("held-tree");
    s.git(
        &repo,
        &["worktree", "add", "-qb", "victim", held.to_str().unwrap()],
    );
    s.delete(&repo, "victim", json!({"force":true}), true);
    assert!(s.exists(&repo, "victim"));
    let repo = s.seed("only", &[]);
    s.rpc_raw(&repo, "delete_branch_preview", json!({"name":"main"}), true);
    s.rpc_raw(
        &repo,
        "delete_branch",
        json!({"name":"main","force":true}),
        true,
    );
    assert!(s.exists(&repo, "main"));
    let repo = s.seed("broken-index", &[]);
    s.git(&repo, &["branch", "victim"]);
    let index = repo.join(".git/index");
    fs::write(&index, b"corrupt index preserved").unwrap();
    let config = s.root.join("registry.json");
    fs::write(config.with_extension("preferences.json"), "{bad").unwrap();
    let result = s.run(
        env!("CARGO_BIN_EXE_desktop-rs"),
        [
            OsString::from("-C"),
            repo.as_os_str().into(),
            "--config".into(),
            config.as_os_str().into(),
            "delete-branch".into(),
            "victim".into(),
        ],
        None,
        false,
    );
    let result: Value = serde_json::from_slice(&result.stdout).unwrap();
    s.rpc(
        &repo,
        "restore_deleted_branch",
        json!({"receipt_id":result["receipt_id"]}),
    );
    assert_eq!(fs::read(index).unwrap(), b"corrupt index preserved");
    for label in ["sha256", "split", "linked", "reftable", "packed"] {
        let options: Vec<&str> = match label {
            "sha256" => vec!["--object-format=sha256"],
            "reftable" => vec!["--ref-format=reftable"],
            _ => vec![],
        };
        let mut repo = s.seed(label, &options);
        s.git(&repo, &["branch", "victim"]);
        if label == "split" {
            s.git(&repo, &["update-index", "--split-index"]);
        }
        if label == "linked" {
            let linked = s.root.join("linked-tree");
            s.git(
                &repo,
                &["worktree", "add", "-qb", "other", linked.to_str().unwrap()],
            );
            repo = linked;
        }
        if label == "packed" {
            s.git(&repo, &["pack-refs", "--all", "--prune"]);
        }
        let result = s.result(&repo, "victim", json!({}));
        assert!(!s.exists(&repo, "victim"));
        s.rpc(
            &repo,
            "restore_deleted_branch",
            json!({"receipt_id":result["receipt_id"]}),
        );
        assert!(s.exists(&repo, "victim"));
    }
    let repo = s.seed("malformed", &[]);
    s.git(&repo, &["branch", "victim"]);
    let result = s.result(&repo, "victim", json!({}));
    let id = result["receipt_id"].as_str().unwrap();
    let metadata = repo
        .join(".git/desktop-branch-deletions")
        .join(format!("{id}.json"));
    let original = fs::read(&metadata).unwrap();
    let outside = s.root.join("outside");
    fs::write(&outside, &original).unwrap();
    fs::remove_file(&metadata).unwrap();
    std::os::unix::fs::symlink(&outside, &metadata).unwrap();
    assert!(s.entries(&repo)[0]["error"].is_string());
    s.rpc_raw(
        &repo,
        "forget_deleted_branch",
        json!({"receipt_id":id}),
        true,
    );
    assert_eq!(fs::read(&outside).unwrap(), original);
    fs::remove_file(&metadata).unwrap();
    s.run("mkfifo", [metadata.as_os_str()], None, false);
    assert!(s.entries(&repo)[0]["error"].is_string());
    fs::remove_file(&metadata).unwrap();
    fs::write(&metadata, vec![b'x'; 1024 * 1024 + 1]).unwrap();
    assert!(
        s.entries(&repo)[0]["error"]
            .as_str()
            .unwrap()
            .contains("size")
    );
    fs::write(&metadata, original).unwrap();
    let new = s.git(
        &repo,
        &["commit-tree", "HEAD^{tree}", "-m", "changed backup"],
    );
    s.git(
        &repo,
        &["update-ref", result["backup_ref"].as_str().unwrap(), &new],
    );
    s.rpc_raw(
        &repo,
        "forget_deleted_branch",
        json!({"receipt_id":id}),
        true,
    );
    assert!(metadata.exists());
    let repo = s.seed("symbolic-backup", &[]);
    s.git(&repo, &["branch", "victim"]);
    let result = s.result(&repo, "victim", json!({}));
    s.git(
        &repo,
        &[
            "symbolic-ref",
            result["backup_ref"].as_str().unwrap(),
            "refs/heads/main",
        ],
    );
    s.rpc(
        &repo,
        "forget_deleted_branch",
        json!({"receipt_id":result["receipt_id"]}),
    );
    assert!(s.exists(&repo, "main"));
    assert_eq!(s.git(&repo, &["symbolic-ref", "HEAD"]), "refs/heads/main");
}

#[test]
fn hooks_native_races_and_failed_checkout_remain_recoverable() {
    use std::os::unix::fs::PermissionsExt;
    let mut s = Suite::new();
    let repo = s.seed("configured-hooks", &[]);
    s.git(&repo, &["branch", "victim"]);
    let log = s.root.join("configured-log");
    let hook = repo.join(".git/hooks/reference-transaction");
    let script = "#!/bin/sh\ndata=$(cat)\ncase \"$data\" in *\" refs/heads/victim\"*)\n if [ $# -gt 1 ]; then tag=configured; phase=$2; else tag=file; phase=$1; fi\n printf '%s\\0%s\\0%s\\n' \"$tag\" \"$phase\" \"$data\" >> \"$DELETE_HOOK_LOG\";;\nesac\n";
    fs::write(&hook, script).unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
    s.env
        .insert("DELETE_HOOK_LOG".into(), log.as_os_str().into());
    s.git(
        &repo,
        &["config", "hook.extra.event", "reference-transaction"],
    );
    s.git(
        &repo,
        &[
            "config",
            "hook.extra.command",
            &format!("{} configured", hook.display()),
        ],
    );
    s.git(&repo, &["branch", "-D", "victim"]);
    let expected = fs::read(&log).unwrap();
    s.git(&repo, &["branch", "victim"]);
    fs::write(&log, b"").unwrap();
    s.result(&repo, "victim", json!({"force":true}));
    assert_eq!(fs::read(&log).unwrap(), expected);
    assert!(
        s.git_result(&repo, &["config", "--get", "hook.extra.enabled"], true)
            .stdout
            .is_empty()
    );
    let repo = s.seed("quoted-hooks", &[]);
    s.git(&repo, &["branch", "victim'quoted"]);
    let directory = s.root.join("hooks space'and\nnewline");
    fs::create_dir(&directory).unwrap();
    let log = s.root.join("quoted-hook-log");
    let hook = directory.join("reference-transaction");
    fs::write(
        &hook,
        "#!/bin/sh\ndata=$(cat)\nprintf '%s\\0%s\\n' \"$1\" \"$data\" >> \"$DELETE_HOOK_LOG\"\n",
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
    s.env
        .insert("DELETE_HOOK_LOG".into(), log.as_os_str().into());
    s.git(
        &repo,
        &["config", "core.hooksPath", directory.to_str().unwrap()],
    );
    s.result(&repo, "victim'quoted", json!({}));
    let events = fs::read_to_string(&log).unwrap();
    for phase in ["prepared", "committed"] {
        let marker = format!("{phase}\0");
        assert!(events.match_indices(&marker).any(|(index, _)| {
            events[index + marker.len()..]
                .split('\0')
                .next()
                .unwrap()
                .contains("refs/heads/victim'quoted\n")
        }));
    }
    assert_eq!(
        s.git_result(&repo, &["config", "core.hooksPath"], false)
            .stdout,
        format!("{}\n", directory.display()).as_bytes()
    );
    s.env.remove(OsStr::new("DELETE_HOOK_LOG"));
    let real = String::from_utf8(s.run("sh", ["-c", "command -v git"], None, false).stdout)
        .unwrap()
        .trim()
        .to_owned();
    for label in ["disabled-hooks", "race"] {
        let repo = s.seed(label, &[]);
        s.git(&repo, &["branch", "victim"]);
        if label == "disabled-hooks" {
            s.git(
                &repo,
                &["config", "hook.reference-transaction.enabled", "false"],
            );
        } else {
            s.git(&repo, &["config", "branch.victim.description", "retained"]);
        }
        let old = s.git(&repo, &["rev-parse", "victim"]);
        let new = s.git(&repo, &["commit-tree", "HEAD^{tree}", "-m", "concurrent"]);
        let fake = s.root.join(format!("{label}-wrapper"));
        fs::create_dir(&fake).unwrap();
        let tool = fake.join("git");
        fs::write(&tool, "#!/bin/sh\ncase \" $* \" in *\" branch \"*\"-D \"*victim*) \"$DELETE_REAL_GIT\" -C \"$DELETE_REPO\" update-ref refs/heads/victim \"$DELETE_NEW_TIP\" || exit 95;; esac\nexec \"$DELETE_REAL_GIT\" \"$@\"\n").unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o700)).unwrap();
        let path = std::env::join_paths(std::iter::once(fake).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )))
        .unwrap();
        for (key, value) in [
            ("PATH", path),
            ("DELETE_REAL_GIT", real.clone().into()),
            ("DELETE_REPO", repo.as_os_str().into()),
            ("DELETE_NEW_TIP", new.clone().into()),
        ] {
            s.env.insert(key.into(), value);
        }
        let output = s.delete(&repo, "victim", json!({"force":true}), true);
        if label == "race" {
            assert!(String::from_utf8_lossy(&output.stderr).contains("tip changed"));
        }
        for key in ["PATH", "DELETE_REAL_GIT", "DELETE_REPO", "DELETE_NEW_TIP"] {
            s.env.remove(OsStr::new(key));
        }
        assert!(s.exists(&repo, "victim"));
        assert_eq!(s.git(&repo, &["rev-parse", "victim"]), new);
        if label == "disabled-hooks" {
            assert_eq!(
                s.git(
                    &repo,
                    &["config", "--bool", "hook.reference-transaction.enabled"]
                ),
                "false"
            );
        } else {
            assert_eq!(
                s.git(&repo, &["config", "branch.victim.description"]),
                "retained"
            );
            assert_eq!(s.entries(&repo)[0]["backup_oid"], old);
            assert_eq!(s.entries(&repo)[0]["phase"], "deleting");
        }
    }
    let repo = s.seed("refusal", &[]);
    s.git(&repo, &["branch", "victim"]);
    let hook = repo.join(".git/hooks/reference-transaction");
    fs::write(&hook, "#!/bin/sh\ndata=$(cat)\ncase \"$1:$data\" in prepared:*\" refs/heads/victim\"*) exit 72;; esac\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
    s.delete(&repo, "victim", json!({"force":true}), true);
    assert!(s.exists(&repo, "victim"));
    assert_eq!(s.entries(&repo)[0]["phase"], "deleting");
    let repo = s.seed("checkout-failure", &[]);
    s.git(&repo, &["switch", "-qc", "victim"]);
    fs::write(repo.join("unique"), "unique").unwrap();
    s.commit(&repo);
    fs::write(repo.join("file"), "saved working").unwrap();
    fs::write(repo.join("loose"), "saved loose").unwrap();
    let hook = repo.join(".git/hooks/post-checkout");
    fs::write(&hook, "#!/bin/sh\nexit 65\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
    s.delete(
        &repo,
        "victim",
        json!({"force":true,"strategy":LEAVE}),
        true,
    );
    let record = s.entries(&repo)[0].clone();
    assert_eq!(record["phase"], "switching");
    assert_eq!(record["backup_oid"], record["oid"]);
    assert!(s.exists(&repo, "victim"));
    assert_eq!(s.rpc(&repo, "snapshot", json!({}))["branch"], "main");
    let history = s.rpc(&repo, "checkout_history", json!({}));
    let related = history
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["receipt"]["id"] == record["checkout_receipt_id"])
        .unwrap();
    let stash = &related["saved_stashes"][0];
    assert_eq!(
        s.git_result(
            &repo,
            &["show", &format!("{}:file", stash["oid"].as_str().unwrap())],
            false
        )
        .stdout,
        b"saved working"
    );
    assert_eq!(
        s.git_result(
            &repo,
            &[
                "show",
                &format!("{}:loose", stash["parents"][2].as_str().unwrap())
            ],
            false
        )
        .stdout,
        b"saved loose"
    );
    let repo = s.seed("rebase", &[]);
    s.git(&repo, &["switch", "-qc", "victim"]);
    fs::write(repo.join("file"), "victim").unwrap();
    s.commit(&repo);
    s.git(&repo, &["switch", "-q", "main"]);
    fs::write(repo.join("file"), "main").unwrap();
    s.commit(&repo);
    s.git(&repo, &["switch", "-q", "victim"]);
    s.git_result(&repo, &["rebase", "main"], true);
    let index = fs::read(repo.join(".git/index")).unwrap();
    let working = fs::read(repo.join("file")).unwrap();
    s.delete(&repo, "victim", json!({"force":true}), true);
    assert!(s.exists(&repo, "victim"));
    assert_eq!(fs::read(repo.join(".git/index")).unwrap(), index);
    assert_eq!(fs::read(repo.join("file")).unwrap(), working);
}

#[test]
fn cancellation_locks_and_lfs_preserve_recovery() {
    use std::{
        os::unix::fs::PermissionsExt,
        time::{Duration, Instant},
    };
    let mut s = Suite::new();
    let repo = s.seed("cancel", &[]);
    s.git(&repo, &["branch", "victim"]);
    let marker = s.root.join("deleted");
    let wrapper_dir = s.root.join("cancel-wrapper");
    fs::create_dir(&wrapper_dir).unwrap();
    let wrapper = wrapper_dir.join("git");
    fs::write(&wrapper, "#!/bin/sh\n\"$DELETE_REAL_GIT\" \"$@\"\nresult=$?\ncase \" $* \" in *\" branch \"*\"-D \"*victim*) if [ \"$result\" = 0 ]; then printf deleted > \"$DELETE_MARKER\"; sleep 60; fi;; esac\nexit \"$result\"\n").unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let real = String::from_utf8(s.run("sh", ["-c", "command -v git"], None, false).stdout)
        .unwrap()
        .trim()
        .to_owned();
    let path = std::env::join_paths(std::iter::once(wrapper_dir).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    for (key, value) in [
        ("PATH", path),
        ("DELETE_REAL_GIT", real.into()),
        ("DELETE_MARKER", marker.as_os_str().into()),
    ] {
        s.env.insert(key.into(), value);
    }
    let mut child = s
        .command(env!("CARGO_BIN_EXE_desktop-rs"))
        .args([
            "-C",
            repo.to_str().unwrap(),
            "delete-branch",
            "victim",
            "--force",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !marker.exists() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("native deletion did not reach cancellation marker");
        }
        assert!(
            child.try_wait().unwrap().is_none(),
            "deletion exited before marker"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("cancelled deletion did not stop");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let result = child.wait_with_output().unwrap();
    assert_eq!(
        result.status.code(),
        Some(130),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    for key in ["PATH", "DELETE_REAL_GIT", "DELETE_MARKER"] {
        s.env.remove(OsStr::new(key));
    }
    let record = s.entries(&repo)[0].clone();
    assert_eq!(record["phase"], "deleting");
    assert_eq!(record["backup_oid"], record["oid"]);
    assert!(record["current_oid"].is_null());
    s.rpc(
        &repo,
        "restore_deleted_branch",
        json!({"receipt_id":record["id"]}),
    );
    assert!(s.exists(&repo, "victim"));
    let repo = s.seed("lock", &[]);
    s.git(&repo, &["branch", "victim"]);
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(repo.join(".git/desktop-stash.lock"))
        .unwrap();
    lock.lock().unwrap();
    let mut child = s
        .command(env!("CARGO_BIN_EXE_desktop-rs"))
        .args(["-C", repo.to_str().unwrap(), "delete-branch", "victim"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert!(child.try_wait().unwrap().is_none());
    assert!(s.exists(&repo, "victim"));
    drop(lock);
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("deletion did not resume after releasing the lock");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    if let Some(tool) = std::env::var_os("GIT_LFS_BIN") {
        let directory = PathBuf::from(tool).parent().unwrap().to_path_buf();
        let path = std::env::join_paths(std::iter::once(directory).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )))
        .unwrap();
        s.env.insert("PATH".into(), path);
    }
    let lfs = s
        .command("git")
        .args(["lfs", "version"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    if !lfs.success() {
        assert!(
            std::env::var_os("DESKTOP_TEST_REQUIRE_LFS").is_none(),
            "Git LFS is required for this test"
        );
        return;
    }
    let repo = s.seed("lfs", &[]);
    s.rpc(&repo, "lfs_install", json!({}));
    s.git(&repo, &["lfs", "track", "*.lfs"]);
    let payload = repo.join("data.lfs");
    fs::write(&payload, b"base").unwrap();
    s.commit(&repo);
    s.git(&repo, &["switch", "-qc", "victim"]);
    fs::write(&payload, b"index").unwrap();
    s.git(&repo, &["add", "--", "data.lfs"]);
    fs::write(&payload, b"working").unwrap();
    fs::write(repo.join("new.lfs"), b"untracked").unwrap();
    let result = s.result(&repo, "victim", json!({"strategy":LEAVE}));
    s.rpc(
        &repo,
        "restore_deleted_branch",
        json!({"receipt_id":result["receipt_id"]}),
    );
    let saved = s.rpc(&repo, "stash_list", json!({}));
    s.rpc(
        &repo,
        "stash_apply",
        json!({"oid":saved["entries"][0]["oid"],"restore_index":true}),
    );
    assert_eq!(fs::read(payload).unwrap(), b"working");
    assert_eq!(fs::read(repo.join("new.lfs")).unwrap(), b"untracked");
}
