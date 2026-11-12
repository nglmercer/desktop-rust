//! Remote branch deletion against local Git servers.
#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    os::unix::fs::{PermissionsExt, symlink},
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
            std::env::temp_dir().join(format!("desktop-remote-delete-{}", uuid::Uuid::new_v4()));
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
    fn publish(&self, repo: &Path, remote: &Path, branch: &str) {
        self.git(
            repo,
            &[
                "push",
                "-q",
                remote.to_str().unwrap(),
                &format!("HEAD:refs/heads/{branch}"),
            ],
        );
    }
    fn preview(&self, repo: &Path, request: &Value) -> Value {
        self.rpc(
            repo,
            "remote_branch_delete_preview",
            json!({"request": request}),
        )
    }
    fn delete(&self, repo: &Path, request: &Value, plan: &Value, fail: bool) -> Output {
        self.rpc_raw(
            repo,
            "delete_remote_branch",
            json!({"request":request,"basis":plan["basis"]}),
            fail,
        )
    }
    fn history(&self, repo: &Path) -> Value {
        self.rpc(repo, "remote_deletion_history", json!({}))
    }
}
impl Drop for Suite {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn multiple_destinations_preserve_worktree_and_recover_tips() {
    let s = Suite::new();
    let repo = s.seed("multi", &[]);
    let fetch = s.bare("fetch");
    let a = s.bare("a");
    let b = s.bare("b");
    s.publish(&repo, &fetch, "victim");
    s.publish(&repo, &a, "victim");
    let tip_a = s.git(&repo, &["rev-parse", "HEAD"]);
    fs::write(repo.join("different"), "remote b").unwrap();
    s.commit(&repo);
    let tip_b = s.git(&repo, &["rev-parse", "HEAD"]);
    s.publish(&repo, &b, "victim");
    s.git(
        &repo,
        &["remote", "add", "team/origin", fetch.to_str().unwrap()],
    );
    for remote in [&a, &b] {
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
    s.git(&repo, &["fetch", "-q", "team/origin"]);
    s.git(&repo, &["config", "branch.main.remote", "team/origin"]);
    s.git(&repo, &["config", "branch.main.merge", "refs/heads/victim"]);
    assert_eq!(
        s.rpc(&repo, "remote_deletion_targets", json!({})),
        json!({"remotes":["team/origin"],"upstreams":[{"local_ref":"refs/heads/main","remote":"team/origin","branch":"refs/heads/victim"}]})
    );
    fs::write(repo.join("file"), "staged").unwrap();
    s.git(&repo, &["add", "--", "file"]);
    fs::write(repo.join("file"), "working").unwrap();
    fs::write(repo.join("loose"), "untracked").unwrap();
    let index = fs::read(repo.join(".git/index")).unwrap();
    let config = fs::read(repo.join(".git/config")).unwrap();
    let status = s
        .git_result(&repo, &["status", "--porcelain=v2", "-z"], false)
        .stdout;
    let request = json!({"remote":"team/origin","branch":"refs/heads/victim"});
    let plan = s.preview(&repo, &request);
    assert_eq!(plan["endpoints"][0]["expected"], tip_a);
    assert_eq!(plan["endpoints"][1]["expected"], tip_b);
    assert!(!repo.join(".git/desktop-remote-branch-deletions").exists());
    let result: Value =
        serde_json::from_slice(&s.delete(&repo, &request, &plan, false).stdout).unwrap();
    let receipt = &result["receipt_id"];
    let record = s.history(&repo);
    assert_eq!(record[0]["phase"], "complete");
    for (index, tip) in [&tip_a, &tip_b].into_iter().enumerate() {
        assert_eq!(record[0]["endpoints"][index]["phase"], "deleted");
        assert_eq!(record[0]["endpoints"][index]["backup_oid"], *tip);
    }
    assert_eq!(s.head(&fetch, "victim"), Some(tip_a.clone()));
    assert_eq!(s.head(&a, "victim"), None);
    assert_eq!(s.head(&b, "victim"), None);
    assert_eq!(fs::read(repo.join(".git/index")).unwrap(), index);
    assert_eq!(fs::read(repo.join(".git/config")).unwrap(), config);
    assert_eq!(
        s.git_result(&repo, &["status", "--porcelain=v2", "-z"], false)
            .stdout,
        status
    );
    s.git_result(
        &repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            "refs/remotes/team/origin/victim",
        ],
        true,
    );
    s.git(&repo, &["reflog", "expire", "--expire=now", "--all"]);
    s.git(&repo, &["gc", "--prune=now"]);
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
        "restore_remote_tip",
        json!({"receipt_id":receipt,"endpoint":0,"name":"dangling-restore"}),
        true,
    );
    assert_eq!(s.head(&repo, "leave-absent"), None);
    assert_eq!(
        s.git(&repo, &["symbolic-ref", "refs/heads/dangling-restore"]),
        "refs/heads/leave-absent"
    );
    s.git(
        &repo,
        &["config", "branch.occupied.description", "existing"],
    );
    s.rpc_raw(
        &repo,
        "restore_remote_tip",
        json!({"receipt_id":receipt,"endpoint":0,"name":"occupied"}),
        true,
    );
    assert_eq!(
        s.git(&repo, &["config", "branch.occupied.description"]),
        "existing"
    );
    for (index, tip) in [&tip_a, &tip_b].into_iter().enumerate() {
        let name = format!("restored-{index}");
        s.rpc(
            &repo,
            "restore_remote_tip",
            json!({"receipt_id":receipt,"endpoint":index,"name":name}),
        );
        assert_eq!(s.git(&repo, &["rev-parse", &name]), *tip);
    }
    s.rpc_raw(
        &repo,
        "restore_remote_tip",
        json!({"receipt_id":receipt,"endpoint":0,"name":"restored-0"}),
        true,
    );
    s.rpc(
        &repo,
        "forget_remote_deletion",
        json!({"receipt_id":receipt}),
    );
    assert_eq!(s.history(&repo), json!([]));
    assert_eq!(s.git(&repo, &["symbolic-ref", "HEAD"]), "refs/heads/main");
    assert_eq!(
        s.git_result(&repo, &["status", "--porcelain=v2", "-z"], false)
            .stdout,
        status
    );
}

#[test]
fn stale_reviews_and_server_policy_do_not_delete_tips() {
    let s = Suite::new();
    for kind in ["tip", "config", "cache"] {
        let repo = s.seed(&format!("stale-{kind}"), &[]);
        let remote = s.bare(&format!("server-{kind}"));
        s.publish(&repo, &remote, "victim");
        s.git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        s.git(&repo, &["fetch", "-q", "origin"]);
        let request = json!({"remote":"origin","branch":"victim"});
        let plan = s.preview(&repo, &request);
        let newer = s.git(
            &repo,
            &["commit-tree", "HEAD^{tree}", "-p", "HEAD", "-m", "outside"],
        );
        match kind {
            "tip" => {
                s.git(
                    &repo,
                    &[
                        "push",
                        "-q",
                        remote.to_str().unwrap(),
                        &format!("{newer}:refs/heads/victim"),
                    ],
                );
            }
            "config" => {
                s.git(&repo, &["config", "remote.origin.proxy", "changed"]);
            }
            _ => {
                s.git(&repo, &["update-ref", "refs/remotes/origin/victim", &newer]);
            }
        }
        s.delete(&repo, &request, &plan, true);
        assert_eq!(
            s.head(&remote, "victim"),
            Some(if kind == "tip" {
                newer
            } else {
                s.git(&repo, &["rev-parse", "HEAD"])
            })
        );
        assert!(!repo.join(".git/desktop-remote-branch-deletions").exists());
    }
    let repo = s.seed("policy", &[]);
    let remote = s.bare("policy-server");
    s.publish(&repo, &remote, "victim");
    s.git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    s.git(&remote, &["config", "receive.denyDeletes", "true"]);
    let request = json!({"remote":"origin","branch":"victim"});
    let plan = s.preview(&repo, &request);
    s.delete(&repo, &request, &plan, true);
    assert_eq!(
        s.head(&remote, "victim"),
        Some(s.git(&repo, &["rev-parse", "HEAD"]))
    );
    for invalid in [
        json!({"remote":"origin","branch":"refs/tags/victim"}),
        json!({"remote":remote,"branch":"victim"}),
        json!({"remote":"origin","branch":"victim","expected":["abcd"]}),
        json!({"remote":"origin","branch":"victim","expected":[]}),
    ] {
        s.rpc_raw(
            &repo,
            "remote_branch_delete_preview",
            json!({"request":invalid}),
            true,
        );
    }
}

#[test]
fn refspec_mapping_and_url_rewrites_follow_git_configuration() {
    let s = Suite::new();
    for kind in ["custom", "negative", "heads"] {
        let repo = s.seed(&format!("refspec-{kind}"), &[]);
        let remote = s.bare(&format!("refspec-server-{kind}"));
        s.publish(&repo, &remote, "victim");
        s.git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        s.git(&repo, &["config", "--unset-all", "remote.origin.fetch"]);
        let destination = if kind == "heads" {
            "refs/heads/keep"
        } else {
            "refs/cache/team/victim"
        };
        let mapping = if kind == "heads" {
            "+refs/heads/victim:refs/heads/keep"
        } else {
            "+refs/heads/*:refs/cache/team/*"
        };
        s.git(&repo, &["config", "--add", "remote.origin.fetch", mapping]);
        if kind == "negative" {
            s.git(
                &repo,
                &[
                    "config",
                    "--add",
                    "remote.origin.fetch",
                    "^refs/heads/victim",
                ],
            );
        }
        let tip = s.git(&repo, &["rev-parse", "HEAD"]);
        s.git(&repo, &["update-ref", destination, &tip]);
        let request = json!({"remote":"origin","branch":"victim"});
        let plan = s.preview(&repo, &request);
        assert_eq!(
            !plan["cached_refs"].as_array().unwrap().is_empty(),
            kind == "custom"
        );
        s.delete(&repo, &request, &plan, false);
        let exists = s
            .command("git")
            .args([
                "-C",
                repo.to_str().unwrap(),
                "show-ref",
                "--verify",
                "--quiet",
                destination,
            ])
            .status()
            .unwrap()
            .success();
        assert_eq!(exists, kind != "custom");
    }
    for kind in ["instead", "push-instead", "fallback", "reset"] {
        let repo = s.seed(&format!("rewrite-{kind}"), &[]);
        let remote = s.bare(&format!("rewrite-server-{kind}"));
        s.publish(&repo, &remote, "victim");
        s.git(&repo, &["remote", "add", "origin", "alias:repo"]);
        let key = format!(
            "url.{}.{}",
            remote.display(),
            if kind == "push-instead" {
                "pushInsteadOf"
            } else {
                "insteadOf"
            }
        );
        s.git(&repo, &["config", &key, "alias:repo"]);
        if kind == "fallback" {
            s.git(
                &repo,
                &[
                    "config",
                    "--add",
                    "remote.origin.url",
                    remote.to_str().unwrap(),
                ],
            );
        }
        if kind == "reset" {
            for url in ["/never-use", "", remote.to_str().unwrap()] {
                s.git(&repo, &["config", "--add", "remote.origin.pushurl", url]);
            }
        }
        let request = json!({"remote":"origin","branch":"victim"});
        let plan = s.preview(&repo, &request);
        s.delete(&repo, &request, &plan, false);
        assert_eq!(s.head(&remote, "victim"), None);
    }
}

#[test]
fn foreign_backups_absent_tips_and_explicit_leases() {
    let s = Suite::new();
    let repo = s.seed("fetch-backup", &[]);
    let writer = s.seed("writer", &[]);
    fs::write(writer.join("foreign"), "server-only").unwrap();
    s.commit(&writer);
    let remote = s.bare("foreign-server");
    s.publish(&writer, &remote, "victim");
    s.git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    let index = fs::read(repo.join(".git/index")).unwrap();
    let request = json!({"remote":"origin","branch":"victim","fetch_backup":true});
    let plan = s.preview(&repo, &request);
    let result: Value =
        serde_json::from_slice(&s.delete(&repo, &request, &plan, false).stdout).unwrap();
    let record = s.history(&repo);
    assert_eq!(
        record[0]["endpoints"][0]["backup_oid"],
        s.git(&writer, &["rev-parse", "HEAD"])
    );
    assert!(!repo.join(".git/FETCH_HEAD").exists());
    assert_eq!(fs::read(repo.join(".git/index")).unwrap(), index);
    s.rpc(
        &repo,
        "restore_remote_tip",
        json!({"receipt_id":result["receipt_id"],"endpoint":0,"name":"retained-foreign"}),
    );
    assert_eq!(
        s.git(&repo, &["show", "retained-foreign:foreign"]),
        "server-only"
    );
    s.git(
        &repo,
        &[
            "symbolic-ref",
            record[0]["endpoints"][0]["backup_ref"].as_str().unwrap(),
            "refs/heads/main",
        ],
    );
    s.rpc_raw(
        &repo,
        "forget_remote_deletion",
        json!({"receipt_id":result["receipt_id"]}),
        true,
    );
    assert_eq!(
        s.head(&repo, "main"),
        Some(s.git(&repo, &["rev-parse", "HEAD"]))
    );

    let repo = s.seed("absent", &[]);
    let remote = s.bare("absent-server");
    s.git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    let tip = s.git(&repo, &["rev-parse", "HEAD"]);
    s.git(&repo, &["update-ref", "refs/remotes/origin/victim", &tip]);
    let request = json!({"remote":"origin","branch":"victim"});
    let plan = s.preview(&repo, &request);
    s.delete(&repo, &request, &plan, false);
    assert_eq!(
        s.history(&repo)[0]["endpoints"][0]["phase"],
        "already_missing"
    );
    s.publish(&repo, &remote, "victim");
    let request = json!({"remote":"origin","branch":"victim","expected":[null]});
    let plan = s.preview(&repo, &request);
    s.delete(&repo, &request, &plan, true);
    assert_eq!(s.head(&remote, "victim"), Some(tip));
}

#[test]
fn nested_refs_mirror_and_missing_backups_are_scoped() {
    let s = Suite::new();
    let repo = s.seed("nested", &[]);
    let remote = s.bare("nested-server");
    s.publish(&repo, &remote, "refs/heads/topic");
    s.git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    assert!(
        s.preview(&repo, &json!({"remote":"origin","branch":"topic"}))["endpoints"][0]["expected"]
            .is_null()
    );
    let request = json!({"remote":"origin","branch":"refs/heads/refs/heads/topic"});
    let plan = s.preview(&repo, &request);
    s.delete(&repo, &request, &plan, false);
    assert_eq!(s.head(&remote, "refs/heads/topic"), None);

    let repo = s.seed("mirror", &[]);
    let remote = s.bare("mirror-server");
    s.publish(&repo, &remote, "victim");
    s.publish(&repo, &remote, "keep");
    s.git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    s.git(&repo, &["config", "remote.origin.mirror", "true"]);
    s.git(&repo, &["config", "push.followTags", "true"]);
    s.git(&repo, &["tag", "-am", "local tag", "local-tag"]);
    let request = json!({"remote":"origin","branch":"victim"});
    let plan = s.preview(&repo, &request);
    s.delete(&repo, &request, &plan, false);
    assert_eq!(s.head(&remote, "victim"), None);
    assert_eq!(
        s.head(&remote, "keep"),
        Some(s.git(&repo, &["rev-parse", "HEAD"]))
    );
    s.git_result(
        &remote,
        &["show-ref", "--verify", "--quiet", "refs/tags/local-tag"],
        true,
    );

    let writer = s.seed("writer-no-backup", &[]);
    fs::write(writer.join("foreign"), "unavailable").unwrap();
    s.commit(&writer);
    let repo = s.seed("no-backup", &[]);
    let remote = s.bare("no-backup-server");
    s.publish(&writer, &remote, "victim");
    s.git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    let request = json!({"remote":"origin","branch":"victim"});
    let plan = s.preview(&repo, &request);
    assert_eq!(plan["endpoints"][0]["available_locally"], false);
    let result: Value =
        serde_json::from_slice(&s.delete(&repo, &request, &plan, false).stdout).unwrap();
    assert!(!result["warnings"].as_array().unwrap().is_empty());
    assert!(s.history(&repo)[0]["endpoints"][0]["backup_oid"].is_null());
    s.rpc(
        &repo,
        "forget_remote_deletion",
        json!({"receipt_id":result["receipt_id"]}),
    );
    assert_eq!(s.history(&repo), json!([]));
}

#[test]
fn corrupt_index_is_untouched_and_transport_errors_hide_credentials() {
    let s = Suite::new();
    let repo = s.seed("corrupt", &[]);
    let remote = s.bare("corrupt-server");
    s.publish(&repo, &remote, "victim");
    s.git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    fs::write(repo.join(".git/index"), b"not an index").unwrap();
    let request = json!({"remote":"origin","branch":"victim"});
    let plan = s.preview(&repo, &request);
    s.delete(&repo, &request, &plan, false);
    assert_eq!(fs::read(repo.join(".git/index")).unwrap(), b"not an index");

    let repo = s.seed("url-errors", &[]);
    s.git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "http://private-user:private-secret@127.0.0.1:1/repo?token=private-token",
        ],
    );
    let error = s.rpc_raw(
        &repo,
        "remote_branch_delete_preview",
        json!({"request":{"remote":"origin","branch":"victim"}}),
        true,
    );
    let request = json!({"remote":"origin","branch":"victim","expected":[s.git(&repo, &["rev-parse", "HEAD"])]});
    let plan = s.preview(&repo, &request);
    s.delete(&repo, &request, &plan, true);
    let public = s.history(&repo).to_string();
    let stderr = String::from_utf8(error.stderr).unwrap();
    for secret in ["private-user", "private-secret", "private-token"] {
        assert!(!stderr.contains(secret));
        assert!(!public.contains(secret));
    }
}

fn executable(path: &Path, source: &str) {
    fs::write(path, source).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn pre_push_race_retains_other_destination_and_cached_update() {
    let mut s = Suite::new();
    let repo = s.seed("partial", &[]);
    let a = s.bare("partial-a");
    let b = s.bare("partial-b");
    s.publish(&repo, &a, "victim");
    s.publish(&repo, &b, "victim");
    let tip = s.git(&repo, &["rev-parse", "HEAD"]);
    s.git(&repo, &["remote", "add", "origin", a.to_str().unwrap()]);
    for remote in [&a, &b] {
        s.git(
            &repo,
            &[
                "config",
                "--add",
                "remote.origin.pushurl",
                remote.to_str().unwrap(),
            ],
        );
    }
    s.git(&repo, &["fetch", "-q", "origin"]);
    let newer = s.git(
        &repo,
        &["commit-tree", "HEAD^{tree}", "-p", "HEAD", "-m", "outside"],
    );
    s.git(
        &repo,
        &[
            "push",
            "-q",
            b.to_str().unwrap(),
            &format!("{newer}:refs/heads/other"),
        ],
    );
    let hooks = s.root.join("hooks with 'quote\nand newline");
    fs::create_dir(&hooks).unwrap();
    executable(
        &hooks.join("pre-push"),
        "#!/bin/sh\nset -e\nprintf '%s\\n%s\\n' \"$1\" \"$2\" >> \"$HOOK_ARGS\"\ncat >> \"$HOOK_STDIN\"\nif [ \"$2\" = \"$RACE_REMOTE\" ]; then\n git -C \"$RACE_REMOTE\" update-ref refs/heads/victim \"$RACE_TIP\"\n git -C \"$RACE_REPO\" update-ref refs/remotes/origin/victim \"$RACE_TIP\"\nfi\n",
    );
    for (key, value) in [
        ("HOOK_ARGS", s.root.join("args").into_os_string()),
        ("HOOK_STDIN", s.root.join("stdin").into_os_string()),
        ("RACE_REMOTE", b.as_os_str().into()),
        ("RACE_REPO", repo.as_os_str().into()),
        ("RACE_TIP", newer.clone().into()),
    ] {
        s.env.insert(key.into(), value);
    }
    s.git(
        &repo,
        &["config", "core.hooksPath", hooks.to_str().unwrap()],
    );
    let request = json!({"remote":"origin","branch":"victim"});
    let plan = s.preview(&repo, &request);
    s.delete(&repo, &request, &plan, true);
    let record = s.history(&repo);
    assert_eq!(s.head(&a, "victim"), None);
    assert_eq!(s.head(&b, "victim"), Some(newer.clone()));
    assert_eq!(record[0]["endpoints"][0]["phase"], "deleted");
    assert_eq!(record[0]["endpoints"][1]["phase"], "failed");
    assert_eq!(
        s.git(&repo, &["rev-parse", "refs/remotes/origin/victim"]),
        newer
    );
    assert_eq!(
        fs::read_to_string(s.root.join("args")).unwrap(),
        format!("origin\n{}\norigin\n{}\n", a.display(), b.display())
    );
    let input = fs::read_to_string(s.root.join("stdin")).unwrap();
    assert_eq!(
        input.matches(&format!("refs/heads/victim {tip}\n")).count(),
        2
    );
}

#[test]
fn native_file_and_configured_pre_push_hooks_receive_same_payload() {
    let mut s = Suite::new();
    let repo = s.seed("configured", &[]);
    let remote = s.bare("configured-server");
    s.publish(&repo, &remote, "victim");
    s.git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    s.git(&repo, &["fetch", "-q", "origin"]);
    let hook = repo.join(".git/hooks/pre-push");
    executable(
        &hook,
        "#!/bin/sh\nif [ \"$1\" = configured ]; then shift; out=\"$CONFIGURED_LOG\"; else out=\"$FILE_LOG\"; fi\n[ \"$1\" = origin ] || exit 0\nprintf '%s\\n%s\\n' \"$1\" \"$2\" >> \"$out.args\"\ncat >> \"$out.input\"\n",
    );
    s.env.insert(
        "CONFIGURED_LOG".into(),
        s.root.join("configured").into_os_string(),
    );
    s.env
        .insert("FILE_LOG".into(), s.root.join("file-hook").into_os_string());
    s.git(&repo, &["config", "hook.extra.event", "pre-push"]);
    s.git(
        &repo,
        &[
            "config",
            "--add",
            "hook.extra.event",
            "reference-transaction",
        ],
    );
    s.git(
        &repo,
        &[
            "config",
            "hook.extra.command",
            &format!("'{}' configured", hook.display()),
        ],
    );
    let request = json!({"remote":"origin","branch":"victim"});
    let plan = s.preview(&repo, &request);
    s.delete(&repo, &request, &plan, false);
    for suffix in ["args", "input"] {
        let file = fs::read(s.root.join(format!("file-hook.{suffix}"))).unwrap();
        let configured = fs::read(s.root.join(format!("configured.{suffix}"))).unwrap();
        assert_eq!(file, configured);
        assert!(!file.is_empty());
    }
    assert_eq!(
        fs::read_to_string(s.root.join("configured.args")).unwrap(),
        format!("origin\n{}\n", remote.display())
    );
}

#[test]
fn interrupted_push_records_uncertainty_and_rejects_unsafe_journals() {
    let mut s = Suite::new();
    let repo = s.seed("cancel", &[]);
    let remote = s.bare("cancel-server");
    s.publish(&repo, &remote, "victim");
    s.git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    let marker = s.root.join("remote-mutated");
    s.env
        .insert("REMOTE_MARKER".into(), marker.as_os_str().into());
    executable(
        &remote.join("hooks/post-receive"),
        "#!/bin/sh\nprintf ready > \"$REMOTE_MARKER\"\nsleep 60\n",
    );
    let request = json!({"remote":"origin","branch":"victim"});
    let plan = s.preview(&repo, &request);
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
        .write_all(
            json!({"method":"delete_remote_branch","request":request,"basis":plan["basis"]})
                .to_string()
                .as_bytes(),
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !marker.exists() && Instant::now() < deadline {
        assert!(
            child.try_wait().unwrap().is_none(),
            "push exited before remote mutation"
        );
        thread::sleep(Duration::from_millis(20));
    }
    if !marker.exists() {
        let _ = child.kill();
        let _ = child.wait();
        panic!("remote mutation timed out");
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
            panic!("interrupted push did not exit");
        }
        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        status
            .code()
            .or_else(|| status.signal().map(|signal| 128 + signal)),
        Some(130)
    );
    assert_eq!(s.head(&remote, "victim"), None);
    let records = s.history(&repo);
    let record = &records[0];
    assert_eq!(record["phase"], "pushing");
    assert_eq!(record["endpoints"][0]["phase"], "pushing");
    assert_eq!(
        record["endpoints"][0]["backup_oid"],
        s.git(&repo, &["rev-parse", "HEAD"])
    );
    assert!(!records.to_string().contains(s.root.to_str().unwrap()));
    let journal = repo
        .join(".git/desktop-remote-branch-deletions")
        .join(format!("{}.json", record["id"].as_str().unwrap()));
    let original = fs::read(&journal).unwrap();
    fs::remove_file(&journal).unwrap();
    symlink(s.root.join("global"), &journal).unwrap();
    assert!(s.history(&repo)[0]["error"].is_string());
    fs::remove_file(&journal).unwrap();
    s.run("mkfifo", [journal.as_os_str()], None, false);
    // Bound this read so a regression cannot leave the test runner hanging on a FIFO.
    let input = json!({"method":"remote_deletion_history"}).to_string();
    let output = s.run(
        "timeout",
        [
            OsString::from("5"),
            env!("CARGO_BIN_EXE_desktop-rs").into(),
            "-C".into(),
            repo.as_os_str().into(),
            "repo".into(),
            "-".into(),
        ],
        Some(input.as_bytes()),
        false,
    );
    let fifo_records: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(fifo_records[0]["error"].is_string());
    fs::remove_file(&journal).unwrap();
    fs::write(&journal, vec![b'x'; 1024 * 1024 + 1]).unwrap();
    assert!(s.history(&repo)[0]["error"].is_string());
    fs::write(&journal, original).unwrap();
}

#[test]
fn supplied_expectations_work_without_server_reads() {
    let mut s = Suite::new();
    let repo = s.seed("push-only", &[]);
    let remote = s.bare("push-only-server");
    s.publish(&repo, &remote, "victim");
    s.git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    let tip = s.git(&repo, &["rev-parse", "HEAD"]);
    let real_git = String::from_utf8(s.run("sh", ["-c", "command -v git"], None, false).stdout)
        .unwrap()
        .trim()
        .to_owned();
    let shim = s.root.join("shim");
    fs::create_dir(&shim).unwrap();
    s.env.insert("REAL_GIT".into(), real_git.into());
    let mut paths = vec![shim.clone()];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    s.env
        .insert("PATH".into(), std::env::join_paths(paths).unwrap());
    executable(
        &shim.join("git"),
        "#!/bin/sh\nread_remote=0\nget_url=0\nfor arg do\n [ \"$arg\" != ls-remote ] || read_remote=1\n [ \"$arg\" != --get-url ] || get_url=1\ndone\n[ \"$read_remote\" != 1 ] || [ \"$get_url\" = 1 ] || exit 87\nexec \"$REAL_GIT\" \"$@\"\n",
    );
    let request = json!({"remote":"origin","branch":"victim","expected":[tip]});
    let plan = s.preview(&repo, &request);
    assert_eq!(plan["supplied_expectations"], true);
    s.delete(&repo, &request, &plan, false);
    assert_eq!(s.head(&remote, "victim"), None);
    s.rpc_raw(
        &repo,
        "remote_branch_delete_preview",
        json!({"request":{"remote":"origin","branch":"victim"}}),
        true,
    );
}

#[test]
fn native_ref_formats_and_linked_worktrees_restore_backups() {
    let s = Suite::new();
    for kind in ["sha256", "reftable", "linked"] {
        let options: &[&str] = match kind {
            "sha256" => &["--object-format=sha256"],
            "reftable" => &["--ref-format=reftable"],
            _ => &[],
        };
        let mut repo = s.seed(&format!("native-{kind}"), options);
        let remote = s.root.join(format!("native-server-{kind}"));
        let mut args = vec![OsString::from("init"), "-q".into(), "--bare".into()];
        if kind == "sha256" {
            args.push("--object-format=sha256".into());
        }
        args.push(remote.as_os_str().into());
        s.run("git", args, None, false);
        s.publish(&repo, &remote, "victim");
        s.git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        if kind == "linked" {
            let linked = s.root.join("linked-worktree");
            s.git(
                &repo,
                &["worktree", "add", "-qb", "linked", linked.to_str().unwrap()],
            );
            repo = linked;
        }
        let request = json!({"remote":"origin","branch":"victim"});
        let plan = s.preview(&repo, &request);
        let result: Value =
            serde_json::from_slice(&s.delete(&repo, &request, &plan, false).stdout).unwrap();
        s.rpc(
            &repo,
            "restore_remote_tip",
            json!({"receipt_id":result["receipt_id"],"endpoint":0,"name":"restore-native"}),
        );
        assert_eq!(
            s.head(&repo, "restore-native"),
            Some(s.git(&repo, &["rev-parse", "HEAD"]))
        );
    }
}

#[test]
fn lfs_branch_tip_can_be_recovered() {
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
        eprintln!("Git LFS unavailable; skipping LFS recovery case");
        return;
    }
    let repo = s.seed("lfs", &[]);
    let remote = s.bare("lfs-server");
    s.git(&repo, &["lfs", "install", "--local"]);
    s.git(&repo, &["lfs", "track", "*.lfs"]);
    fs::write(repo.join("payload.lfs"), b"native LFS branch tip").unwrap();
    s.commit(&repo);
    s.git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    s.git(&repo, &["push", "-q", "origin", "HEAD:refs/heads/victim"]);
    let request = json!({"remote":"origin","branch":"victim"});
    let plan = s.preview(&repo, &request);
    let result: Value =
        serde_json::from_slice(&s.delete(&repo, &request, &plan, false).stdout).unwrap();
    assert_eq!(s.head(&remote, "victim"), None);
    s.rpc(
        &repo,
        "restore_remote_tip",
        json!({"receipt_id":result["receipt_id"],"endpoint":0,"name":"restored-lfs"}),
    );
    assert!(
        s.git(&repo, &["show", "restored-lfs:payload.lfs"])
            .starts_with("version https://git-lfs.github.com/spec/v1")
    );
}
