//! Default-branch policy and action guards against isolated native refs.
#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("desktop-branch-policy-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }
    fn command(&self, program: &str) -> Command {
        let mut cmd = Command::new(program);
        cmd.env("HOME", &self.root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.root.join("global.config"))
            .env("GIT_TERMINAL_PROMPT", "0")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS");
        cmd
    }
    fn exec(&self, program: &str, args: &[OsString], input: Option<&str>, fail: bool) -> Output {
        let mut cmd = self.command(program);
        cmd.args(args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        if let Some(input) = input {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
        }
        let result = child.wait_with_output().unwrap();
        assert_eq!(
            result.status.success(),
            !fail,
            "{program} {args:?}: stdout={} stderr={}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        result
    }
    fn git_result(&self, repo: &Path, args: &[&str], fail: bool) -> Output {
        let mut all = vec![OsString::from("-C"), repo.as_os_str().to_owned()];
        all.extend(args.iter().map(OsString::from));
        self.exec("git", &all, None, fail)
    }
    fn git(&self, repo: &Path, args: &[&str]) -> String {
        String::from_utf8(self.git_result(repo, args, false).stdout)
            .unwrap()
            .trim()
            .to_owned()
    }
    fn seed(&self, name: &str, options: &[&str]) -> PathBuf {
        let repo = self.root.join(name);
        let mut args: Vec<OsString> = ["init", "-q", "-b", "main"]
            .into_iter()
            .map(Into::into)
            .collect();
        args.extend(options.iter().map(OsString::from));
        args.push(repo.as_os_str().into());
        self.exec("git", &args, None, false);
        self.git(&repo, &["config", "user.name", "Policy Test"]);
        self.git(&repo, &["config", "user.email", "policy@example.invalid"]);
        self.git(&repo, &["config", "commit.gpgsign", "false"]);
        fs::write(repo.join("file"), "base").unwrap();
        self.git(&repo, &["add", "--all"]);
        self.git(&repo, &["commit", "-qm", "initial"]);
        repo
    }
    fn rpc_raw(&self, repo: &Path, method: &str, mut body: Value, fail: bool) -> Output {
        body["method"] = json!(method);
        self.exec(
            env!("CARGO_BIN_EXE_desktop-rs"),
            &[
                "-C".into(),
                repo.as_os_str().into(),
                "repo".into(),
                "-".into(),
            ],
            Some(&body.to_string()),
            fail,
        )
    }
    fn rpc(&self, repo: &Path, method: &str, body: Value) -> Value {
        serde_json::from_slice(&self.rpc_raw(repo, method, body, false).stdout).unwrap()
    }
    fn policy(&self, repo: &Path) -> Value {
        self.rpc(repo, "branch_policy", json!({}))
    }
    fn permission(&self, repo: &Path, name: &str) -> Value {
        self.policy(repo)["branches"]
            .as_array()
            .unwrap()
            .iter()
            .find(|branch| branch["name"] == name)
            .unwrap()
            .clone()
    }
    fn remote(&self, repo: &Path, name: &str, default: &str) {
        self.git(
            repo,
            &[
                "remote",
                "add",
                name,
                self.root.join("uncontacted.git").to_str().unwrap(),
            ],
        );
        let tip = self.git(repo, &["rev-parse", "HEAD"]);
        self.git(
            repo,
            &[
                "update-ref",
                &format!("refs/remotes/{name}/{default}"),
                &tip,
            ],
        );
        self.git(
            repo,
            &[
                "symbolic-ref",
                &format!("refs/remotes/{name}/HEAD"),
                &format!("refs/remotes/{name}/{default}"),
            ],
        );
    }
    fn reject_delete(&self, repo: &Path, name: &str) {
        let before = self.git(repo, &["for-each-ref", "--format=%(refname) %(objectname)"]);
        for (method, body) in [
            ("delete_branch_preview", json!({"name":name})),
            ("delete_branch", json!({"name":name,"force":true})),
        ] {
            let output = self.rpc_raw(repo, method, body, true);
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains("default branch cannot be deleted")
            );
        }
        assert_eq!(
            self.git(repo, &["for-each-ref", "--format=%(refname) %(objectname)"]),
            before
        );
        assert!(!repo.join(".git/desktop-branch-deletions").exists());
    }
    fn reject_rename(&self, repo: &Path, name: &str) {
        for method in ["rename_branch_preview", "rename_branch"] {
            let output = self.rpc_raw(
                repo,
                method,
                json!({"name":name,"new_name":"replacement"}),
                true,
            );
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains("published default branch cannot be renamed")
            );
        }
        assert!(!repo.join(".git/desktop-branch-renames").exists());
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn fallback_global_and_remote_selection() {
    let f = Fixture::new();
    let repo = f.seed("fallback", &[]);
    f.git(&repo, &["branch", "feature"]);
    fs::write(repo.join("file"), "working").unwrap();
    fs::write(repo.join("untracked"), b"keep\0").unwrap();
    let index = fs::read(repo.join(".git/index")).unwrap();
    let policy = f.policy(&repo);
    assert_eq!(policy["source"], "main");
    assert!(policy["default_remote"].is_null());
    assert_eq!(
        policy["default_branch"],
        json!({"name":"main","reference":"refs/heads/main","local":true})
    );
    let permission = f.permission(&repo, "main");
    assert_eq!(permission["can_rename"], true);
    assert_eq!(permission["can_delete"], false);
    f.reject_delete(&repo, "main");
    assert_eq!(fs::read(repo.join(".git/index")).unwrap(), index);
    assert_eq!(fs::read(repo.join("file")).unwrap(), b"working");
    assert_eq!(fs::read(repo.join("untracked")).unwrap(), b"keep\0");
    let listed = f.rpc(&repo, "branch_list", json!({}));
    assert_eq!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["name"] == "main")
            .unwrap()["policy"],
        permission
    );
    let snapshot = f.rpc(&repo, "snapshot", json!({}));
    assert_eq!(snapshot["branch_policy"], permission);
    assert_eq!(snapshot["default_branch"], policy["default_branch"]);
    let review = f.rpc(
        &repo,
        "rename_branch_preview",
        json!({"name":"main","new_name":"renamed"}),
    );
    assert_eq!(review["policy"]["is_default"], true);
    assert_eq!(review["policy"]["published"], false);
    f.rpc(
        &repo,
        "rename_branch",
        json!({"name":"main","new_name":"renamed","basis":review["basis"]}),
    );
    assert!(f.policy(&repo)["default_branch"].is_null());
    f.git(&repo, &["branch", "main"]);
    f.reject_delete(&repo, "main");

    let repo = f.seed("global", &[]);
    let wanted = "release/ñ'$(literal)";
    f.git(&repo, &["branch", wanted]);
    f.git(&repo, &["config", "--global", "init.defaultBranch", wanted]);
    f.git(&repo, &["config", "init.defaultBranch", "main"]);
    let policy = f.policy(&repo);
    assert_eq!(policy["source"], "global_init");
    assert_eq!(policy["default_branch"]["name"], wanted);
    f.reject_delete(&repo, wanted);
    assert_eq!(f.permission(&repo, "main")["can_delete"], true);
    f.git(
        &repo,
        &["config", "--global", "--unset-all", "init.defaultBranch"],
    );
    let repo = f.seed("global-includes", &[]);
    f.git(&repo, &["branch", "trunk"]);
    let included = f.root.join("included-default");
    fs::write(&included, "[init]\n    defaultBranch = trunk\n").unwrap();
    f.git(
        &repo,
        &[
            "config",
            "--global",
            "include.path",
            included.to_str().unwrap(),
        ],
    );
    assert_eq!(f.policy(&repo)["source"], "main");
    assert_eq!(f.policy(&repo)["default_branch"]["name"], "main");
    f.reject_delete(&repo, "main");
    assert_eq!(f.permission(&repo, "trunk")["can_delete"], true);
    f.git(
        &repo,
        &["config", "--global", "--unset-all", "include.path"],
    );
    let include = format!("includeIf.gitdir:{}/.git", repo.display());
    f.git(
        &repo,
        &[
            "config",
            "--global",
            &format!("{include}.path"),
            included.to_str().unwrap(),
        ],
    );
    assert_eq!(
        f.git_result(
            &repo,
            &[
                "config",
                "--global",
                "--includes",
                "--null",
                "--get",
                "init.defaultBranch"
            ],
            false
        )
        .stdout,
        b"trunk\0"
    );
    assert_eq!(f.policy(&repo)["default_branch"]["name"], "main");
    f.git(&repo, &["config", "--global", "--remove-section", &include]);

    let repo = f.seed("remotes", &[]);
    f.remote(&repo, "aaa", "other");
    f.git(&repo, &["branch", "other"]);
    f.remote(&repo, "origin", "release/stable");
    f.git(&repo, &["branch", "release/stable"]);
    let policy = f.policy(&repo);
    assert_eq!(policy["default_remote"], "origin");
    assert_eq!(policy["default_name"], "release/stable");
    assert_eq!(policy["source"], "remote_head");
    f.reject_delete(&repo, "release/stable");
    f.git(&repo, &["remote", "remove", "origin"]);
    assert_eq!(f.policy(&repo)["default_branch"]["name"], "other");
    f.git(
        &repo,
        &["symbolic-ref", "--delete", "refs/remotes/aaa/HEAD"],
    );
    assert_eq!(f.policy(&repo)["default_branch"]["name"], "main");
    let repo = f.seed("fetch-capable", &[]);
    f.remote(&repo, "aaa", "other");
    f.git(&repo, &["branch", "other"]);
    f.git(&repo, &["config", "remote.origin.pushurl", "not-contacted"]);
    assert_eq!(f.policy(&repo)["default_remote"], "aaa");
    f.git(
        &repo,
        &[
            "config",
            "--add",
            "remote.origin.url",
            "https://private:secret@example.invalid/repo?token=secret\nsecond line",
        ],
    );
    let tip = f.git(&repo, &["rev-parse", "HEAD"]);
    f.git(&repo, &["update-ref", "refs/remotes/origin/main", &tip]);
    f.git(
        &repo,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    let policy = f.policy(&repo);
    assert_eq!(policy["default_remote"], "origin");
    assert!(!policy.to_string().contains("secret"));
    f.git(&repo, &["config", "--add", "remote.origin.url", ""]);
    assert_eq!(f.policy(&repo)["default_remote"], "aaa");
    f.git(
        &repo,
        &[
            "config",
            "--add",
            "remote.origin.url",
            "not-contacted-again",
        ],
    );
    assert_eq!(f.policy(&repo)["default_remote"], "origin");
    let repo = f.seed("unicode-remotes", &[]);
    f.remote(&repo, "\u{e000}", "bmp");
    f.remote(&repo, "\u{10000}", "release/ñ'quoted");
    assert_eq!(f.policy(&repo)["default_remote"], "\u{10000}");
    assert_eq!(f.policy(&repo)["default_name"], "release/ñ'quoted");
}

#[test]
fn tracking_aliases_review_fences_and_checkout_hooks() {
    let f = Fixture::new();
    let repo = f.seed("tracking", &[]);
    f.remote(&repo, "origin", "main");
    for branch in ["a-track", "z-track"] {
        f.git(&repo, &["branch", branch]);
        f.git(
            &repo,
            &["branch", "--set-upstream-to", "origin/main", branch],
        );
    }
    assert_eq!(f.policy(&repo)["default_branch"]["name"], "a-track");
    f.reject_delete(&repo, "a-track");
    f.reject_rename(&repo, "a-track");
    assert_eq!(f.permission(&repo, "main")["can_delete"], true);
    assert_eq!(f.permission(&repo, "z-track")["can_rename"], true);
    f.git(
        &repo,
        &["branch", "--set-upstream-to", "origin/main", "main"],
    );
    assert_eq!(f.policy(&repo)["default_branch"]["name"], "main");
    f.reject_rename(&repo, "main");
    let repo = f.seed("symbolic-alias", &[]);
    f.remote(&repo, "origin", "main");
    f.git(&repo, &["branch", "feature"]);
    f.git(
        &repo,
        &["symbolic-ref", "refs/heads/aaa", "refs/heads/feature"],
    );
    f.git(&repo, &["config", "branch.aaa.remote", "origin"]);
    f.git(&repo, &["config", "branch.aaa.merge", "refs/heads/main"]);
    let policy = f.policy(&repo);
    assert_eq!(policy["default_branch"]["name"], "main");
    assert!(
        policy["branches"]
            .as_array()
            .unwrap()
            .iter()
            .all(|b| b["name"] != "aaa")
    );
    assert!(
        f.rpc(&repo, "branch_list", json!({}))
            .as_array()
            .unwrap()
            .iter()
            .all(|b| b["name"] != "aaa" && b["name"] != "origin/HEAD")
    );
    f.reject_delete(&repo, "main");
    f.rpc_raw(
        &repo,
        "rename_branch_preview",
        json!({"name":"aaa","new_name":"renamed-alias"}),
        true,
    );
    f.rpc_raw(&repo, "delete_branch_preview", json!({"name":"aaa"}), true);
    assert_eq!(
        f.git(&repo, &["symbolic-ref", "refs/heads/aaa"]),
        "refs/heads/feature"
    );
    f.git(
        &repo,
        &["branch", "--set-upstream-to", "origin/main", "main"],
    );
    f.git(&repo, &["update-ref", "-d", "refs/remotes/origin/main"]);
    assert_eq!(f.permission(&repo, "main")["published"], true);
    f.reject_rename(&repo, "main");
    let repo = f.seed("remote-only", &[]);
    f.git(&repo, &["branch", "-m", "feature"]);
    f.remote(&repo, "origin", "trunk");
    assert_eq!(f.policy(&repo)["default_branch"]["local"], false);
    let review = f.rpc(&repo, "delete_branch_preview", json!({"name":"feature"}));
    assert_eq!(
        review["checkout"]["target"]["reference"],
        "refs/remotes/origin/trunk"
    );
    f.rpc(
        &repo,
        "delete_branch",
        json!({"name":"feature","force":true,"strategy":"MoveToNewBranch","basis":review["basis"]}),
    );
    assert_eq!(f.git(&repo, &["symbolic-ref", "HEAD"]), "refs/heads/trunk");
    assert_eq!(
        f.git(&repo, &["config", "branch.trunk.merge"]),
        "refs/heads/trunk"
    );
    assert_eq!(f.permission(&repo, "trunk")["is_default"], true);
    assert_eq!(f.permission(&repo, "trunk")["can_delete"], false);

    for operation in ["delete", "rename"] {
        for blocked in [true, false] {
            let repo = f.seed(&format!("stale-{operation}-{blocked}"), &[]);
            f.git(&repo, &["branch", "feature"]);
            f.remote(&repo, "origin", "main");
            let tip = f.git(&repo, &["rev-parse", "HEAD"]);
            let target = if blocked { "feature" } else { "other" };
            if !blocked {
                f.git(&repo, &["branch", "other"]);
            }
            f.git(
                &repo,
                &["update-ref", &format!("refs/remotes/origin/{target}"), &tip],
            );
            if blocked {
                f.git(
                    &repo,
                    &["branch", "--set-upstream-to", "origin/feature", "feature"],
                );
            }
            let (preview, action, body) = if operation == "delete" {
                (
                    "delete_branch_preview",
                    "delete_branch",
                    json!({"name":"feature"}),
                )
            } else {
                (
                    "rename_branch_preview",
                    "rename_branch",
                    json!({"name":"feature","new_name":"replacement"}),
                )
            };
            let review = f.rpc(&repo, preview, body.clone());
            f.git(
                &repo,
                &[
                    "symbolic-ref",
                    "refs/remotes/origin/HEAD",
                    &format!("refs/remotes/origin/{target}"),
                ],
            );
            if !blocked {
                let permission = f.permission(&repo, "feature");
                assert_eq!(permission["can_delete"], true);
                assert_eq!(permission["can_rename"], true);
            }
            let mut action_body = body;
            action_body["basis"] = review["basis"].clone();
            if operation == "delete" {
                action_body["force"] = json!(true);
            }
            let output = f.rpc_raw(&repo, action, action_body, true);
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(
                if blocked {
                    error.contains("default branch")
                } else {
                    error.contains("context changed") || error.contains("review changed")
                },
                "{error}"
            );
            f.git(
                &repo,
                &["show-ref", "--verify", "--quiet", "refs/heads/feature"],
            );
            assert!(!repo.join(".git/desktop-branch-deletions").exists());
            assert!(!repo.join(".git/desktop-branch-renames").exists());
        }
    }
    let repo = f.seed("checkout-hook", &[]);
    f.git(&repo, &["switch", "-qc", "feature"]);
    f.remote(&repo, "origin", "main");
    let tip = f.git(&repo, &["rev-parse", "HEAD"]);
    f.git(&repo, &["update-ref", "refs/remotes/origin/feature", &tip]);
    let hook = repo.join(".git/hooks/post-checkout");
    fs::write(
        &hook,
        "#!/bin/sh\ngit symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/feature\n",
    )
    .unwrap();
    fs::set_permissions(hook, fs::Permissions::from_mode(0o700)).unwrap();
    let review = f.rpc(&repo, "delete_branch_preview", json!({"name":"feature"}));
    let result = f.rpc_raw(
        &repo,
        "delete_branch",
        json!({"name":"feature","force":true,"basis":review["basis"],"strategy":"MoveToNewBranch"}),
        true,
    );
    assert!(String::from_utf8_lossy(&result.stderr).contains("default branch cannot be deleted"));
    f.git(
        &repo,
        &["show-ref", "--verify", "--quiet", "refs/heads/feature"],
    );
    assert_eq!(
        f.rpc(&repo, "deleted_branches", json!({}))[0]["phase"],
        "switching"
    );
}

#[test]
fn native_ref_formats_detached_and_corrupt_state() {
    let f = Fixture::new();
    for (label, options) in [
        ("sha256", vec!["--object-format=sha256"]),
        ("reftable", vec!["--ref-format=reftable"]),
        ("packed", vec![]),
        ("linked", vec![]),
    ] {
        let mut repo = f.seed(label, &options);
        f.remote(&repo, "origin", "main");
        f.git(
            &repo,
            &["branch", "--set-upstream-to", "origin/main", "main"],
        );
        if label == "packed" {
            f.git(&repo, &["pack-refs", "--all", "--prune"]);
        }
        if label == "linked" {
            let linked = f.root.join("linked-tree");
            f.git(
                &repo,
                &[
                    "worktree",
                    "add",
                    "-qb",
                    "feature",
                    linked.to_str().unwrap(),
                ],
            );
            repo = linked;
        }
        f.reject_delete(&repo, "main");
        f.reject_rename(&repo, "main");
        f.git(&repo, &["switch", "--detach", "-q"]);
        assert!(f.rpc(&repo, "snapshot", json!({}))["branch_policy"].is_null());
        let gitdir = PathBuf::from(f.git(&repo, &["rev-parse", "--absolute-git-dir"]));
        let index = gitdir.join("index");
        fs::write(&index, b"corrupt preserved").unwrap();
        let registry = f.root.join(format!("{label}-registry.json"));
        let preferences = registry.with_extension("preferences.json");
        fs::write(&preferences, "{broken").unwrap();
        let result = f.exec(
            env!("CARGO_BIN_EXE_desktop-rs"),
            &[
                "-C".into(),
                repo.as_os_str().into(),
                "--config".into(),
                registry.as_os_str().into(),
                "branch-policy".into(),
            ],
            None,
            false,
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&result.stdout).unwrap()["default_branch"]["name"],
            "main"
        );
        assert!(!registry.exists());
        assert_eq!(fs::read_to_string(preferences).unwrap(), "{broken");
        assert_eq!(fs::read(index).unwrap(), b"corrupt preserved");
    }
    let unborn = f.root.join("unborn");
    f.exec(
        "git",
        &[
            "init".into(),
            "-q".into(),
            "-b".into(),
            "main".into(),
            unborn.as_os_str().into(),
        ],
        None,
        false,
    );
    assert_eq!(f.policy(&unborn)["branches"], json!([]));
    assert!(f.policy(&unborn)["default_branch"].is_null());
    assert!(f.rpc(&unborn, "snapshot", json!({}))["branch_policy"].is_null());
    let repo = f.seed("bad-remote-head", &[]);
    f.remote(&repo, "origin", "main");
    f.git(
        &repo,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/heads/main",
        ],
    );
    assert!(f.policy(&repo)["remote_head"].is_null());
    assert_eq!(f.policy(&repo)["default_branch"]["name"], "main");
    assert_eq!(
        f.rpc(&repo, "snapshot", json!({}))["branch_policy"]["is_default"],
        true
    );
    f.reject_delete(&repo, "main");
    let repo = f.seed("looping-remote-head", &[]);
    f.remote(&repo, "origin", "main");
    f.git(
        &repo,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/HEAD",
        ],
    );
    assert_eq!(
        f.git_result(
            &repo,
            &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
            true
        )
        .status
        .code(),
        Some(128)
    );
    let fallback = f.policy(&repo);
    assert!(fallback["remote_head"].is_null());
    assert_eq!(fallback["source"], "main");
    assert_eq!(fallback["default_branch"]["name"], "main");
    let snapshot = f.rpc(&repo, "snapshot", json!({}));
    assert_eq!(snapshot["default_branch"]["name"], "main");
    assert_eq!(snapshot["branch_policy"]["is_default"], true);
}
