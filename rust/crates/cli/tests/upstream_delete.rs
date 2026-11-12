//! Reviewed combined local and upstream branch deletion.
#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    os::unix::process::ExitStatusExt,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Suite {
    root: PathBuf,
    env: HashMap<OsString, OsString>,
}
impl Suite {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("desktop-upstream-delete-{}", uuid::Uuid::new_v4()));
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
    fn bare(&self, name: &str) -> PathBuf {
        let repo = self.root.join(name);
        self.run(
            "git",
            [
                OsString::from("init"),
                "-q".into(),
                "--bare".into(),
                repo.as_os_str().into(),
            ],
            None,
            false,
        );
        repo
    }
    fn head(&self, repo: &Path, branch: &str) -> Option<String> {
        let output = self
            .command("git")
            .args([
                "-C",
                repo.to_str().unwrap(),
                "show-ref",
                "--verify",
                "--hash",
                &format!("refs/heads/{branch}"),
            ])
            .output()
            .unwrap();
        output
            .status
            .success()
            .then(|| String::from_utf8(output.stdout).unwrap().trim().to_owned())
    }
    fn setup(&self, name: &str) -> (PathBuf, PathBuf) {
        self.setup_with_options(name, &[])
    }
    fn setup_with_options(&self, name: &str, options: &[&str]) -> (PathBuf, PathBuf) {
        let repo = self.seed(name, options);
        let remote = self.root.join(format!("{name}-server"));
        let mut args = vec![OsString::from("init"), "-q".into(), "--bare".into()];
        if options.contains(&"--object-format=sha256") {
            args.push("--object-format=sha256".into());
        }
        args.push(remote.as_os_str().into());
        self.run("git", args, None, false);
        self.git(&repo, &["branch", "local-topic"]);
        self.git(
            &repo,
            &["remote", "add", "team/origin", remote.to_str().unwrap()],
        );
        self.git(
            &repo,
            &[
                "push",
                "-q",
                "team/origin",
                "local-topic:refs/heads/server-topic",
            ],
        );
        self.git(&repo, &["fetch", "-q", "team/origin"]);
        self.git(
            &repo,
            &["config", "branch.local-topic.remote", "team/origin"],
        );
        self.git(
            &repo,
            &[
                "config",
                "branch.local-topic.merge",
                "refs/heads/server-topic",
            ],
        );
        (repo, remote)
    }
    fn preview(&self, repo: &Path, upstream: Value, checkout: Option<&str>) -> Value {
        self.rpc(
            repo,
            "delete_branch_preview",
            json!({"name":"local-topic","upstream":upstream,"to_checkout":checkout}),
        )
    }
    fn delete(
        &self,
        repo: &Path,
        plan: &Value,
        upstream: Value,
        mut extra: Value,
        fail: bool,
    ) -> Output {
        extra["name"] = json!("local-topic");
        extra["upstream"] = upstream;
        extra["basis"] = plan["basis"].clone();
        self.rpc_raw(repo, "delete_branch", extra, fail)
    }
    fn records(&self, repo: &Path) -> (Value, Value) {
        (
            self.rpc(repo, "deleted_branches", json!({})),
            self.rpc(repo, "remote_deletion_history", json!({})),
        )
    }
}
impl Drop for Suite {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn reviewed_deletion_links_recovery_and_preserves_dirty_files() {
    let s = Suite::new();
    let (repo, remote) = s.setup("basic");
    let wrong = s.bare("wrong");
    s.git(&repo, &["remote", "add", "wrong", wrong.to_str().unwrap()]);
    s.git(
        &repo,
        &["push", "-q", "wrong", "HEAD:refs/heads/server-topic"],
    );
    s.git(&repo, &["config", "branch.local-topic.pushRemote", "wrong"]);
    s.git(
        &repo,
        &[
            "config",
            "branch.local-topic.description",
            "saved\nsettings",
        ],
    );
    s.git(
        &repo,
        &[
            "config",
            "branch.local-topic.child.description",
            "other section",
        ],
    );
    let tip = s.head(&repo, "local-topic").unwrap();
    fs::write(repo.join("file"), "staged").unwrap();
    s.git(&repo, &["add", "--", "file"]);
    fs::write(repo.join("file"), "working").unwrap();
    fs::write(repo.join("loose"), "loose").unwrap();
    let index = fs::read(repo.join(".git/index")).unwrap();
    let status = s
        .git_result(&repo, &["status", "--porcelain=v2", "-z"], false)
        .stdout;
    let plan = s.preview(&repo, json!({}), None);
    assert_eq!(plan["upstream"]["remote"], "team/origin");
    assert_eq!(plan["upstream"]["reference"], "refs/heads/server-topic");
    assert!(plan["checkout"].is_null());
    assert!(!repo.join(".git/desktop-branch-deletions").exists());
    assert!(!repo.join(".git/desktop-remote-branch-deletions").exists());
    let result: Value =
        serde_json::from_slice(&s.delete(&repo, &plan, json!({}), json!({}), false).stdout)
            .unwrap();
    let (local, upstream) = s.records(&repo);
    let local = &local[0];
    let upstream = &upstream[0];
    assert_eq!(local["phase"], "complete");
    assert_eq!(upstream["phase"], "complete");
    assert_eq!(result["remote_deletion_receipt_id"], upstream["id"]);
    assert_eq!(local["remote_deletion_receipt_id"], upstream["id"]);
    assert_eq!(upstream["local_deletion_receipt_id"], local["id"]);
    assert_eq!(s.head(&repo, "local-topic"), None);
    assert_eq!(s.head(&remote, "server-topic"), None);
    assert_eq!(s.head(&wrong, "server-topic"), Some(tip.clone()));
    assert_eq!(fs::read(repo.join(".git/index")).unwrap(), index);
    assert_eq!(
        s.git_result(&repo, &["status", "--porcelain=v2", "-z"], false)
            .stdout,
        status
    );
    assert_eq!(
        s.git(&repo, &["config", "branch.local-topic.child.description"]),
        "other section"
    );
    for (dir, record, field, method) in [
        (
            "desktop-branch-deletions",
            local,
            "remote_deletion_receipt_id",
            "deleted_branches",
        ),
        (
            "desktop-remote-branch-deletions",
            upstream,
            "local_deletion_receipt_id",
            "remote_deletion_history",
        ),
    ] {
        let path = repo
            .join(".git")
            .join(dir)
            .join(format!("{}.json", record["id"].as_str().unwrap()));
        let original = fs::read(&path).unwrap();
        let mut old: Value = serde_json::from_slice(&original).unwrap();
        old.as_object_mut().unwrap().remove(field);
        fs::write(&path, old.to_string()).unwrap();
        assert!(s.rpc(&repo, method, json!({}))[0][field].is_null());
        fs::write(path, original).unwrap();
    }
    s.git(&repo, &["reflog", "expire", "--expire=now", "--all"]);
    s.git(&repo, &["gc", "--prune=now"]);
    s.rpc(
        &repo,
        "restore_deleted_branch",
        json!({"receipt_id":local["id"]}),
    );
    s.rpc(
        &repo,
        "restore_remote_tip",
        json!({"receipt_id":upstream["id"],"endpoint":0,"name":"recovered-upstream"}),
    );
    assert_eq!(s.head(&repo, "local-topic"), Some(tip.clone()));
    assert_eq!(s.head(&repo, "recovered-upstream"), Some(tip));
    assert_eq!(
        s.git(&repo, &["config", "branch.local-topic.merge"]),
        "refs/heads/server-topic"
    );
    assert_eq!(
        s.git(&repo, &["config", "branch.local-topic.description"]),
        "saved\nsettings"
    );
    assert_eq!(
        s.git_result(&repo, &["status", "--porcelain=v2", "-z"], false)
            .stdout,
        status
    );
    assert_eq!(s.git(&repo, &["symbolic-ref", "HEAD"]), "refs/heads/main");
}

#[test]
fn stale_reviews_and_unconfigured_upstreams_are_rejected_without_mutation() {
    let s = Suite::new();
    let wrong = s.bare("wrong");
    for kind in [
        "remote-tip",
        "remote-config",
        "upstream",
        "local-ref",
        "options",
    ] {
        let (repo, remote) = s.setup(&format!("stale-{kind}"));
        let plan = s.preview(&repo, json!({}), None);
        let new = s.git(
            &repo,
            &["commit-tree", "HEAD^{tree}", "-p", "HEAD", "-m", "new"],
        );
        match kind {
            "remote-tip" => {
                s.git(
                    &repo,
                    &[
                        "push",
                        "-q",
                        remote.to_str().unwrap(),
                        &format!("{new}:refs/heads/server-topic"),
                    ],
                );
            }
            "remote-config" => {
                s.git(
                    &repo,
                    &[
                        "config",
                        "remote.team/origin.pushurl",
                        wrong.to_str().unwrap(),
                    ],
                );
            }
            "upstream" => {
                s.git(
                    &repo,
                    &["config", "branch.local-topic.merge", "refs/heads/other"],
                );
            }
            "local-ref" => {
                s.git(&repo, &["update-ref", "refs/heads/local-topic", &new]);
            }
            _ => {}
        }
        s.delete(
            &repo,
            &plan,
            if kind == "options" {
                json!({"fetch_backup":true})
            } else {
                json!({})
            },
            json!({}),
            true,
        );
        assert!(s.head(&repo, "local-topic").is_some());
        assert!(s.head(&remote, "server-topic").is_some());
        assert!(!repo.join(".git/desktop-branch-deletions").exists());
    }
    for kind in ["none", "dot"] {
        let repo = s.seed(&format!("no-network-{kind}"), &[]);
        s.git(&repo, &["branch", "local-topic"]);
        if kind == "dot" {
            s.git(&repo, &["config", "branch.local-topic.remote", "."]);
            s.git(
                &repo,
                &["config", "branch.local-topic.merge", "refs/heads/main"],
            );
        }
        s.rpc_raw(
            &repo,
            "delete_branch_preview",
            json!({"name":"local-topic","upstream":{}}),
            true,
        );
        assert!(!repo.join(".git/desktop-branch-deletions").exists());
        s.rpc(&repo, "delete_branch", json!({"name":"local-topic"}));
        assert_eq!(s.head(&repo, "local-topic"), None);
    }
}

#[test]
fn checkout_consent_and_stash_strategies_preserve_changes() {
    let s = Suite::new();
    let (repo, remote) = s.setup("consent");
    s.git(&repo, &["switch", "-q", "local-topic"]);
    fs::write(repo.join("file"), "dirty").unwrap();
    let plan = s.preview(&repo, json!({}), None);
    let rejected = s.delete(&repo, &plan, json!({}), json!({}), true);
    let rejected: Value = serde_json::from_slice(&rejected.stdout).unwrap();
    assert_eq!(rejected["requires_confirmation"], true);
    assert!(s.head(&remote, "server-topic").is_some());
    assert!(!repo.join(".git/desktop-branch-deletions").exists());
    for strategy in ["StashOnCurrentBranch", "MoveToNewBranch"] {
        let (repo, remote) = s.setup(&format!("current-{strategy}"));
        s.git(&repo, &["switch", "-q", "local-topic"]);
        fs::write(repo.join("file"), "index").unwrap();
        s.git(&repo, &["add", "--", "file"]);
        fs::write(repo.join("file"), "working").unwrap();
        fs::write(repo.join("loose"), "untracked").unwrap();
        let plan = s.preview(&repo, json!({}), Some("main"));
        s.delete(
            &repo,
            &plan,
            json!({}),
            json!({"strategy":strategy,"to_checkout":"main"}),
            false,
        );
        let (local, upstream) = s.records(&repo);
        assert_eq!(s.head(&repo, "local-topic"), None);
        assert_eq!(s.head(&remote, "server-topic"), None);
        assert_eq!(s.git(&repo, &["symbolic-ref", "HEAD"]), "refs/heads/main");
        assert!(local[0]["checkout_receipt_id"].is_string());
        assert_eq!(upstream[0]["phase"], "complete");
        if strategy == "StashOnCurrentBranch" {
            let list = s.rpc(&repo, "stash_list", json!({}));
            let stash = &list["entries"][0];
            assert_eq!(stash["branch"], "local-topic");
            let oid = stash["oid"].as_str().unwrap();
            assert_eq!(s.git(&repo, &["show", &format!("{oid}:file")]), "working");
            assert_eq!(
                s.git(
                    &repo,
                    &[
                        "show",
                        &format!("{}:file", stash["parents"][1].as_str().unwrap())
                    ]
                ),
                "index"
            );
            assert_eq!(
                s.git(
                    &repo,
                    &[
                        "show",
                        &format!("{}:loose", stash["parents"][2].as_str().unwrap())
                    ]
                ),
                "untracked"
            );
        } else {
            assert_eq!(fs::read_to_string(repo.join("file")).unwrap(), "working");
            assert_eq!(s.git(&repo, &["show", ":file"]), "index");
            assert_eq!(fs::read_to_string(repo.join("loose")).unwrap(), "untracked");
        }
    }
    let (repo, remote) = s.setup("remote-checkout");
    s.git(
        &repo,
        &["push", "-q", "team/origin", "HEAD:refs/heads/destination"],
    );
    s.git(&repo, &["fetch", "-q", "team/origin"]);
    s.git(&repo, &["switch", "-q", "local-topic"]);
    let target = "refs/remotes/team/origin/destination";
    let plan = s.preview(&repo, json!({}), Some(target));
    s.delete(
        &repo,
        &plan,
        json!({}),
        json!({"to_checkout":target}),
        false,
    );
    assert_eq!(
        s.git(&repo, &["symbolic-ref", "HEAD"]),
        "refs/heads/destination"
    );
    assert_eq!(
        s.git(&repo, &["config", "branch.destination.remote"]),
        "team/origin"
    );
    assert_eq!(s.head(&remote, "server-topic"), None);
}

#[test]
fn upstream_server_refusal_allows_local_recovery_without_mutating_server() {
    let s = Suite::new();
    let (repo, remote) = s.setup("server-refusal");
    s.git(&remote, &["config", "receive.denyDeletes", "true"]);
    let plan = s.preview(&repo, json!({}), None);
    s.delete(&repo, &plan, json!({}), json!({}), true);
    let (local, upstream) = s.records(&repo);
    assert_eq!(s.head(&repo, "local-topic"), None);
    assert!(s.head(&remote, "server-topic").is_some());
    assert_eq!(local[0]["phase"], "deleting_upstream");
    assert_eq!(upstream[0]["endpoints"][0]["phase"], "failed");
    s.rpc(
        &repo,
        "restore_deleted_branch",
        json!({"receipt_id":local[0]["id"]}),
    );
    assert!(s.head(&repo, "local-topic").is_some());
    assert!(s.head(&remote, "server-topic").is_some());
}

fn executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn local_and_checkout_hook_refusals_never_push_upstream() {
    let mut s = Suite::new();
    let (repo, remote) = s.setup("local-refusal");
    let marker = s.root.join("must-not-push");
    s.env
        .insert("PUSH_MARKER".into(), marker.as_os_str().into());
    executable(
        &repo.join(".git/hooks/pre-push"),
        "#!/bin/sh\nprintf pushed > \"$PUSH_MARKER\"\n",
    );
    executable(
        &repo.join(".git/hooks/reference-transaction"),
        "#!/bin/sh\nif [ \"$1\" = prepared ]; then\n while read -r old new ref; do [ \"$ref\" != refs/heads/local-topic ] || exit 72; done\nfi\n",
    );
    let plan = s.preview(&repo, json!({}), None);
    s.delete(&repo, &plan, json!({}), json!({}), true);
    let (_, upstream) = s.records(&repo);
    assert!(s.head(&repo, "local-topic").is_some());
    assert!(s.head(&remote, "server-topic").is_some());
    assert!(!marker.exists());
    assert_eq!(upstream[0]["phase"], "local_deletion_stopped");
    assert_eq!(
        upstream[0]["endpoints"][0]["backup_oid"],
        s.head(&repo, "local-topic").unwrap()
    );

    let (repo, remote) = s.setup("checkout-refusal");
    s.git(&repo, &["switch", "-q", "local-topic"]);
    fs::write(repo.join("file"), "saved working").unwrap();
    executable(
        &repo.join(".git/hooks/post-checkout"),
        "#!/bin/sh\nexit 73\n",
    );
    let plan = s.preview(&repo, json!({}), None);
    s.delete(
        &repo,
        &plan,
        json!({}),
        json!({"strategy":"StashOnCurrentBranch"}),
        true,
    );
    let (local, upstream) = s.records(&repo);
    assert!(s.head(&repo, "local-topic").is_some());
    assert!(s.head(&remote, "server-topic").is_some());
    assert_eq!(s.git(&repo, &["symbolic-ref", "HEAD"]), "refs/heads/main");
    assert_eq!(upstream[0]["phase"], "local_deletion_stopped");
    assert!(
        !s.rpc(&repo, "stash_list", json!({}))["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(local[0]["checkout_receipt_id"].is_string());
}

#[test]
fn public_cli_requires_upstream_opt_in_and_uses_reviewed_basis() {
    let s = Suite::new();
    let (repo, remote) = s.setup("cli");
    let binary = env!("CARGO_BIN_EXE_desktop-rs");
    let expected = s.head(&remote, "server-topic").unwrap();
    let args = ["-C", repo.to_str().unwrap(), "delete-branch", "local-topic"];
    let bad = s
        .command(binary)
        .args(args)
        .arg("--fetch-upstream-backup")
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2));
    assert!(s.head(&repo, "local-topic").is_some());
    let options = [
        "--include-upstream",
        "--fetch-upstream-backup",
        "--expect-upstream",
        &expected,
    ];
    let output = s
        .command(binary)
        .args(args)
        .args(options)
        .arg("--preview")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plan: Value = serde_json::from_slice(&output.stdout).unwrap();
    let result = s
        .command(binary)
        .args(args)
        .args(options)
        .args(["--basis", plan["basis"].as_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(s.head(&repo, "local-topic"), None);
    assert_eq!(s.head(&remote, "server-topic"), None);
}

#[test]
fn interrupted_checkout_and_push_never_replay_server_deletion() {
    for kind in ["checkout", "push"] {
        let mut s = Suite::new();
        let (repo, remote) = s.setup(&format!("cancel-{kind}"));
        let marker = s.root.join("mutation-marker");
        s.env
            .insert("MUTATION_MARKER".into(), marker.as_os_str().into());
        if kind == "checkout" {
            s.git(&repo, &["switch", "-q", "local-topic"]);
            fs::write(repo.join("file"), "saved work").unwrap();
            executable(
                &repo.join(".git/hooks/post-checkout"),
                "#!/bin/sh\nprintf ready > \"$MUTATION_MARKER\"\nsleep 60\n",
            );
        } else {
            executable(
                &remote.join("hooks/post-receive"),
                "#!/bin/sh\nprintf ready > \"$MUTATION_MARKER\"\nsleep 60\n",
            );
        }
        let plan = s.preview(&repo, json!({}), None);
        let mut request = json!({"method":"delete_branch","name":"local-topic","upstream":{},"basis":plan["basis"]});
        if kind == "checkout" {
            request["strategy"] = json!("StashOnCurrentBranch");
        }
        let mut child = s
            .command(env!("CARGO_BIN_EXE_desktop-rs"))
            .args(["-C", repo.to_str().unwrap(), "repo", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(request.to_string().as_bytes())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !marker.exists() && Instant::now() < deadline {
            assert!(
                child.try_wait().unwrap().is_none(),
                "operation exited before hook"
            );
            thread::sleep(Duration::from_millis(20));
        }
        if !marker.exists() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("hook timed out");
        }
        s.run("kill", ["-INT", &child.id().to_string()], None, false);
        let deadline = Instant::now() + Duration::from_secs(15);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("cancelled operation did not exit");
            }
            thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(
            status
                .code()
                .or_else(|| status.signal().map(|signal| 128 + signal)),
            Some(130)
        );
        let (local, upstream) = s.records(&repo);
        if kind == "checkout" {
            assert!(s.head(&repo, "local-topic").is_some());
            assert!(s.head(&remote, "server-topic").is_some());
            assert_eq!(s.git(&repo, &["symbolic-ref", "HEAD"]), "refs/heads/main");
            assert_eq!(local[0]["phase"], "switching");
            assert_eq!(upstream[0]["phase"], "local_deletion_stopped");
            assert!(
                !s.rpc(&repo, "stash_list", json!({}))["entries"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        } else {
            assert_eq!(s.head(&repo, "local-topic"), None);
            assert_eq!(s.head(&remote, "server-topic"), None);
            assert_eq!(local[0]["phase"], "deleting_upstream");
            assert_eq!(upstream[0]["endpoints"][0]["phase"], "pushing");
            s.rpc(
                &repo,
                "restore_deleted_branch",
                json!({"receipt_id":local[0]["id"]}),
            );
            assert!(s.head(&repo, "local-topic").is_some());
            assert_eq!(s.head(&remote, "server-topic"), None);
        }
    }
}

#[test]
fn checkout_rejects_changes_to_transport_and_neighboring_settings() {
    for kind in ["transport", "neighbor", "global"] {
        let mut s = Suite::new();
        let (repo, remote) = s.setup(&format!("during-checkout-{kind}"));
        let wrong = s.bare("wrong");
        s.git(
            &repo,
            &["config", "branch.local-topic.child.description", "before"],
        );
        s.git(&repo, &["switch", "-q", "local-topic"]);
        if kind == "global" {
            s.git(
                &repo,
                &[
                    "config",
                    "--global",
                    "branch.local-topic.description",
                    "before",
                ],
            );
        }
        let (scope, key, value) = match kind {
            "transport" => ("--local", "remote.team/origin.url", wrong.to_str().unwrap()),
            "neighbor" => ("--local", "branch.local-topic.child.description", "after"),
            _ => ("--global", "branch.local-topic.description", "after"),
        };
        s.env.insert("CHANGE_SCOPE".into(), scope.into());
        s.env.insert("CHANGE_KEY".into(), key.into());
        s.env.insert("CHANGE_VALUE".into(), value.into());
        executable(
            &repo.join(".git/hooks/post-checkout"),
            "#!/bin/sh\ngit config \"$CHANGE_SCOPE\" \"$CHANGE_KEY\" \"$CHANGE_VALUE\"\n",
        );
        let plan = s.preview(&repo, json!({}), None);
        s.delete(
            &repo,
            &plan,
            json!({}),
            json!({"strategy":"MoveToNewBranch"}),
            true,
        );
        let (local, upstream) = s.records(&repo);
        assert_eq!(s.head(&repo, "local-topic"), None);
        assert!(s.head(&remote, "server-topic").is_some());
        assert!(local[0]["backup_oid"].is_string());
        assert_eq!(upstream[0]["endpoints"][0]["phase"], "pending");
    }
}

#[test]
fn reviewed_checkout_initializes_submodule_without_changing_transport() {
    let s = Suite::new();
    let (repo, remote) = s.setup("submodules");
    let child = s.seed("submodule-child", &[]);
    s.git(&repo, &["config", "protocol.file.allow", "always"]);
    s.git(
        &repo,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            child.to_str().unwrap(),
            "nested",
        ],
    );
    s.commit(&repo);
    s.git(&repo, &["submodule", "deinit", "-f", "--all"]);
    s.git(&repo, &["switch", "-q", "local-topic"]);
    let plan = s.preview(&repo, json!({}), Some("main"));
    s.delete(
        &repo,
        &plan,
        json!({}),
        json!({"to_checkout":"main","strategy":"MoveToNewBranch"}),
        false,
    );
    assert_eq!(
        fs::read(repo.join("nested/file")).unwrap(),
        fs::read(child.join("file")).unwrap()
    );
    assert_eq!(
        s.git(&repo, &["config", "submodule.nested.url"]),
        child.to_str().unwrap()
    );
    assert_eq!(s.head(&remote, "server-topic"), None);
}

#[test]
fn hook_validate_prepared() {
    let Some(repo) = std::env::var_os("UPSTREAM_HOOK_REPO") else {
        return;
    };
    let repo = PathBuf::from(repo);
    let read_record = |directory: &str| -> Value {
        let mut entries = fs::read_dir(repo.join(".git").join(directory))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"));
        let record = entries.next().unwrap();
        assert!(entries.next().is_none());
        serde_json::from_slice(&fs::read(record).unwrap()).unwrap()
    };
    let local = read_record("desktop-branch-deletions");
    let remote = read_record("desktop-remote-branch-deletions");
    assert_eq!(local["remote_deletion_receipt_id"], remote["id"]);
    assert_eq!(remote["local_deletion_receipt_id"], local["id"]);
    let absent = Command::new("git")
        .args([
            "-C",
            repo.to_str().unwrap(),
            "show-ref",
            "--verify",
            "--quiet",
            "refs/heads/local-topic",
        ])
        .status()
        .unwrap();
    assert_eq!(absent.code(), Some(1));
    for endpoint in remote["endpoints"].as_array().unwrap() {
        let output = Command::new("git")
            .args([
                "-C",
                repo.to_str().unwrap(),
                "show-ref",
                "--verify",
                "--hash",
                endpoint["backup_ref"].as_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().trim(),
            endpoint["expected"].as_str().unwrap()
        );
    }
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::env::var_os("UPSTREAM_HOOK_LOG").unwrap())
        .unwrap();
    writeln!(log, "validated").unwrap();
}

#[test]
fn all_backups_exist_before_first_push_even_with_partial_failure() {
    let mut s = Suite::new();
    let (repo, first) = s.setup("multi");
    let second = s.bare("multi-b");
    s.git(
        &repo,
        &[
            "push",
            "-q",
            second.to_str().unwrap(),
            "HEAD:refs/heads/server-topic",
        ],
    );
    for remote in [&first, &second] {
        s.git(
            &repo,
            &[
                "config",
                "--add",
                "remote.team/origin.pushurl",
                remote.to_str().unwrap(),
            ],
        );
    }
    s.git(&second, &["config", "receive.denyDeletes", "true"]);
    s.env
        .insert("UPSTREAM_HOOK_REPO".into(), repo.as_os_str().into());
    s.env.insert(
        "UPSTREAM_HOOK_LOG".into(),
        s.root.join("hook-log").into_os_string(),
    );
    s.env.insert(
        "UPSTREAM_HOOK_TEST_BINARY".into(),
        std::env::current_exe().unwrap().into_os_string(),
    );
    executable(
        &repo.join(".git/hooks/pre-push"),
        "#!/bin/sh\nexec \"$UPSTREAM_HOOK_TEST_BINARY\" --exact hook_validate_prepared --nocapture >/dev/null\n",
    );
    let plan = s.preview(&repo, json!({}), None);
    s.delete(&repo, &plan, json!({}), json!({}), true);
    let (_, upstream) = s.records(&repo);
    assert_eq!(s.head(&repo, "local-topic"), None);
    assert_eq!(s.head(&first, "server-topic"), None);
    assert!(s.head(&second, "server-topic").is_some());
    assert_eq!(upstream[0]["endpoints"][0]["phase"], "deleted");
    assert_eq!(upstream[0]["endpoints"][1]["phase"], "failed");
    assert_eq!(
        fs::read_to_string(s.root.join("hook-log")).unwrap(),
        "validated\nvalidated\n"
    );
}

#[test]
fn remote_change_during_backup_fetch_stops_before_local_deletion() {
    let mut s = Suite::new();
    let repo = s.seed("backup-fetch", &[]);
    let writer = s.seed("backup-writer", &[]);
    fs::write(writer.join("foreign"), "one").unwrap();
    s.commit(&writer);
    let remote = s.bare("backup-server");
    s.git(
        &writer,
        &[
            "push",
            "-q",
            remote.to_str().unwrap(),
            "HEAD:refs/heads/server-topic",
        ],
    );
    fs::write(writer.join("foreign"), "two").unwrap();
    s.commit(&writer);
    let new = s.git(&writer, &["rev-parse", "HEAD"]);
    s.git(
        &writer,
        &[
            "push",
            "-q",
            remote.to_str().unwrap(),
            "HEAD:refs/heads/other",
        ],
    );
    s.git(&repo, &["branch", "local-topic"]);
    s.git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    s.git(&repo, &["config", "branch.local-topic.remote", "origin"]);
    s.git(
        &repo,
        &[
            "config",
            "branch.local-topic.merge",
            "refs/heads/server-topic",
        ],
    );
    let real_git = String::from_utf8(s.run("sh", ["-c", "command -v git"], None, false).stdout)
        .unwrap()
        .trim()
        .to_owned();
    let shim = s.root.join("fetch-shim");
    fs::create_dir(&shim).unwrap();
    let mut paths = vec![shim.clone()];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    s.env
        .insert("PATH".into(), std::env::join_paths(paths).unwrap());
    s.env.insert("REAL_GIT".into(), real_git.into());
    s.env
        .insert("RACE_REMOTE".into(), remote.as_os_str().into());
    s.env.insert("RACE_TIP".into(), new.clone().into());
    executable(
        &shim.join("git"),
        "#!/bin/sh\nfor arg do\n if [ \"$arg\" = fetch ]; then \"$REAL_GIT\" -C \"$RACE_REMOTE\" update-ref refs/heads/server-topic \"$RACE_TIP\" || exit; break; fi\ndone\nexec \"$REAL_GIT\" \"$@\"\n",
    );
    let upstream = json!({"fetch_backup":true});
    let plan = s.preview(&repo, upstream.clone(), None);
    s.delete(&repo, &plan, upstream, json!({"force":true}), true);
    let (local, history) = s.records(&repo);
    assert!(s.head(&repo, "local-topic").is_some());
    assert_eq!(s.head(&remote, "server-topic"), Some(new.clone()));
    assert_eq!(local[0]["phase"], "backing_up_upstream");
    assert_eq!(history[0]["phase"], "backing_up");
    assert_eq!(history[0]["endpoints"][0]["backup_oid"], new);
}

#[test]
fn linked_worktree_and_native_ref_formats_own_cross_linked_receipts() {
    let s = Suite::new();
    for kind in ["sha256", "reftable", "linked"] {
        let options: &[&str] = match kind {
            "sha256" => &["--object-format=sha256"],
            "reftable" => &["--ref-format=reftable"],
            _ => &[],
        };
        let (mut repo, remote) = s.setup_with_options(&format!("native-{kind}"), options);
        if kind == "linked" {
            let linked = s.root.join("linked");
            s.git(
                &repo,
                &["worktree", "add", "-qb", "linked", linked.to_str().unwrap()],
            );
            repo = linked;
        }
        let plan = s.preview(&repo, json!({}), None);
        s.delete(&repo, &plan, json!({}), json!({}), false);
        let (local, upstream) = s.records(&repo);
        assert_eq!(s.head(&repo, "local-topic"), None);
        assert_eq!(s.head(&remote, "server-topic"), None);
        assert_eq!(local[0]["remote_deletion_receipt_id"], upstream[0]["id"]);
    }
}

#[test]
fn lfs_changes_are_stashed_when_deleting_current_and_upstream_branch() {
    let mut s = Suite::new();
    if let Some(binary) = std::env::var_os("GIT_LFS_BIN") {
        let mut paths = vec![PathBuf::from(binary).parent().unwrap().to_path_buf()];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        s.env
            .insert("PATH".into(), std::env::join_paths(paths).unwrap());
    }
    if !s
        .command("git")
        .args(["lfs", "version"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
    {
        assert!(
            std::env::var_os("DESKTOP_TEST_REQUIRE_LFS").is_none(),
            "Git LFS is required by this test environment"
        );
        eprintln!("Git LFS unavailable; skipping LFS stash case");
        return;
    }
    let (repo, remote) = s.setup("lfs");
    s.git(&repo, &["lfs", "install", "--local"]);
    s.git(&repo, &["lfs", "track", "*.lfs"]);
    fs::write(repo.join("data.lfs"), "base").unwrap();
    s.commit(&repo);
    s.git(&repo, &["switch", "-q", "local-topic"]);
    s.git(&repo, &["merge", "-q", "--ff-only", "main"]);
    s.git(
        &repo,
        &["push", "-q", "team/origin", "HEAD:refs/heads/server-topic"],
    );
    s.git(&repo, &["fetch", "-q", "team/origin"]);
    fs::write(repo.join("data.lfs"), "working").unwrap();
    let plan = s.preview(&repo, json!({}), None);
    s.delete(
        &repo,
        &plan,
        json!({}),
        json!({"strategy":"StashOnCurrentBranch"}),
        false,
    );
    let list = s.rpc(&repo, "stash_list", json!({}));
    let oid = list["entries"][0]["oid"].as_str().unwrap();
    assert!(
        s.git(&repo, &["show", &format!("{oid}:data.lfs")])
            .starts_with("version https://git-lfs.github.com/spec/v1")
    );
    assert_eq!(s.head(&remote, "server-topic"), None);
}
