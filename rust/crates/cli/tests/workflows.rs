//! End-to-end typed Git workflows in isolated repositories.
use serde_json::{Value, json};
#[cfg(unix)]
use std::ffi::OsStr;
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
        Self::with_options(&[])
    }

    fn with_options(options: &[&str]) -> Self {
        let root = std::env::temp_dir().join(format!("desktop-workflows-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let f = Self {
            repo: root.join("repo"),
            root,
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
        f.git(&["config", "user.name", "Migration Test"]);
        f.git(&["config", "user.email", "migration@example.invalid"]);
        f.git(&["config", "commit.gpgsign", "false"]);
        f
    }

    fn command(&self, executable: &str) -> Command {
        let mut command = Command::new(executable);
        command
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_SYSTEM")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.root.join("global"))
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_EDITOR", "true");
        command
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

    fn request(&self, value: Value, success: bool) -> Value {
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
            return serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
        }
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            result.get("code").and_then(Value::as_i64).unwrap_or(0),
            0,
            "{result}"
        );
        result
    }
    fn rpc(&self, value: Value) -> Value {
        self.request(value, true)
    }
    fn fails(&self, value: Value) {
        self.request(value, false);
    }
    fn write(&self, path: &str, contents: &str) {
        fs::write(self.repo.join(path), contents).unwrap();
    }
    fn stage(&self, path: &str) {
        self.rpc(json!({"method":"stage", "paths":[path]}));
    }
    fn commit(&self, summary: &str) {
        self.rpc(json!({"method":"commit", "summary":summary}));
    }
    fn checkout(&self, branch: &str) {
        self.rpc(json!({"method":"checkout", "branch":branch}));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn checkout_fixture(diverge: bool) -> Fixture {
    checkout_fixture_with_options(diverge, &[])
}

#[test]
fn status_context_reads_worktree_markers_and_rebase_metadata() {
    let f = Fixture::new();
    f.write("file", "base\n");
    f.git(&["add", "--", "file"]);
    f.git(&["commit", "-qm", "base"]);
    let clean = f.rpc(json!({"method":"status_context"}));
    assert_eq!(
        clean,
        json!({"mergeHeadFound":false,"squashMsgFound":false,"isCherryPickingHeadFound":false,"rebaseInternalState":null})
    );
    let gitdir = f.repo.join(".git");
    fs::write(gitdir.join("MERGE_HEAD"), b"merge\n").unwrap();
    fs::write(gitdir.join("SQUASH_MSG"), b"squash\n").unwrap();
    fs::write(gitdir.join("CHERRY_PICK_HEAD"), b"pick\n").unwrap();
    fs::write(gitdir.join("REBASE_HEAD"), b"rebase\n").unwrap();
    let rebase = gitdir.join("rebase-merge");
    fs::create_dir(&rebase).unwrap();
    fs::write(rebase.join("orig-head"), b"original\n").unwrap();
    fs::write(rebase.join("head-name"), b"refs/heads/topic\n").unwrap();
    fs::write(rebase.join("onto"), b"base\n").unwrap();
    let context = f.rpc(json!({"method":"status_context"}));
    assert_eq!(
        context,
        json!({"mergeHeadFound":true,"squashMsgFound":true,"isCherryPickingHeadFound":true,"rebaseInternalState":{"originalBranchTip":"original","targetBranch":"topic","baseBranchTip":"base"}})
    );
    #[cfg(unix)]
    {
        fs::remove_file(rebase.join("onto")).unwrap();
        assert!(
            Command::new("mkfifo")
                .arg(rebase.join("onto"))
                .status()
                .unwrap()
                .success()
        );
        assert!(f.rpc(json!({"method":"status_context"}))["rebaseInternalState"].is_null());
        fs::remove_file(rebase.join("onto")).unwrap();
    }
    fs::write(rebase.join("onto"), vec![b'x'; 8193]).unwrap();
    assert!(f.rpc(json!({"method":"status_context"}))["rebaseInternalState"].is_null());
    for marker in [
        "MERGE_HEAD",
        "SQUASH_MSG",
        "CHERRY_PICK_HEAD",
        "REBASE_HEAD",
    ] {
        fs::remove_file(gitdir.join(marker)).unwrap();
    }
    fs::remove_dir_all(rebase).unwrap();
    let linked = f.root.join("linked-context");
    f.git(&["worktree", "add", "-qb", "linked", linked.to_str().unwrap()]);
    let linked_gitdir = f.git_text(&[
        "-C",
        linked.to_str().unwrap(),
        "rev-parse",
        "--absolute-git-dir",
    ]);
    fs::write(
        PathBuf::from(linked_gitdir).join("SQUASH_MSG"),
        b"linked only\n",
    )
    .unwrap();
    assert_eq!(
        f.rpc(json!({"method":"status_context"}))["squashMsgFound"],
        false
    );
    let output = f
        .command(env!("CARGO_BIN_EXE_desktop-rs"))
        .arg("-C")
        .arg(&linked)
        .args(["repo", r#"{"method":"status_context"}"#])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["squashMsgFound"],
        true
    );
}

fn checkout_fixture_with_options(diverge: bool, options: &[&str]) -> Fixture {
    let f = Fixture::with_options(options);
    f.write("file", "base\n");
    f.git(&["add", "--", "file"]);
    f.git(&["commit", "-qm", "base"]);
    f.git(&["switch", "-qc", "other"]);
    if diverge {
        f.write("file", "target\n");
        f.git(&["add", "--", "file"]);
        f.git(&["commit", "-qm", "target"]);
    }
    f.git(&["switch", "-q", "main"]);
    f
}

#[test]
fn checkout_handles_git_storage_variants_and_non_branch_sources() {
    for kind in ["sha256", "split", "linked", "reftable"] {
        let options: &[&str] = match kind {
            "sha256" => &["--object-format=sha256"],
            "reftable" => &["--ref-format=reftable"],
            _ => &[],
        };
        let mut f = checkout_fixture_with_options(true, options);
        if kind == "split" {
            f.git(&["update-index", "--split-index"]);
        }
        if kind == "linked" {
            let linked = f.root.join("linked source");
            f.git(&["worktree", "add", "-qb", "linked", linked.to_str().unwrap()]);
            f.repo = linked;
        }
        f.write("file", "saved branch data");
        let result = reviewed_checkout(&f, "other", "StashOnCurrentBranch", json!({}), true);
        assert!(
            result["saved_oid"]
                .as_str()
                .is_some_and(|id| !id.is_empty()),
            "{kind}"
        );
        assert!(
            !f.rpc(json!({"method":"checkout_history"}))[0]["saved_stashes"]
                .as_array()
                .unwrap()
                .is_empty(),
            "{kind}"
        );
    }
    let f = checkout_fixture(true);
    f.git(&["switch", "--detach", "-q"]);
    f.write("only-new", "carry detached");
    assert_eq!(
        f.rpc(json!({"method":"checkout_preview", "branch":"other"}))["can_leave"],
        false
    );
    reviewed_checkout(&f, "other", "StashOnCurrentBranch", json!({}), true);
    assert_eq!(
        fs::read(f.repo.join("only-new")).unwrap(),
        b"carry detached"
    );
    assert!(
        f.rpc(json!({"method":"stash_list"}))["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let f = checkout_fixture(true);
    f.git(&["switch", "--orphan", "unborn", "-q"]);
    f.write("new", "carry unborn");
    assert_eq!(
        f.rpc(json!({"method":"checkout_preview", "branch":"other"}))["can_leave"],
        false
    );
    reviewed_checkout(&f, "other", "StashOnCurrentBranch", json!({}), true);
    assert_eq!(fs::read(f.repo.join("new")).unwrap(), b"carry unborn");
}

fn reviewed_checkout(
    f: &Fixture,
    branch: &str,
    strategy: &str,
    options: Value,
    success: bool,
) -> Value {
    let plan = f.rpc(json!({"method":"checkout_preview", "branch":branch}));
    let mut request =
        json!({"method":"checkout", "branch":branch, "strategy":strategy, "basis":plan["basis"]});
    request
        .as_object_mut()
        .unwrap()
        .extend(options.as_object().unwrap().clone());
    f.request(request, success)
}

fn stash_selection(f: &Fixture, index: usize) -> Value {
    let list = f.rpc(json!({"method":"stash_list"}));
    json!({"oid":list["entries"][index]["oid"], "index":list["entries"][index]["index"], "basis":list["basis"]})
}

fn selected_stash_request(method: &str, selection: &Value) -> Value {
    let mut request = selection.clone();
    request["method"] = json!(method);
    request
}

#[cfg(unix)]
#[test]
fn stash_conflicts_and_storage_variants_preserve_saved_content() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    f.write("file", "one\ntwo\n");
    f.git(&["add", "--", "file"]);
    f.git(&["commit", "-qm", "base"]);
    f.write("file", "stashed version\n");
    let saved = f.rpc(json!({"method":"stash_save","message":"saved"}));
    f.write("file", "committed other version\n");
    f.git(&["add", "--", "file"]);
    f.git(&["commit", "-qm", "other"]);
    let mut request = selected_stash_request("stash_apply", &stash_selection(&f, 0));
    request["pop"] = json!(true);
    let failed = f.request(request, false);
    assert_eq!(failed["retained"], true);
    assert!(!f.git(&["ls-files", "-u"]).stdout.is_empty());
    assert_eq!(stash_selection(&f, 0)["oid"], saved["oid"]);
    f.write("file", "resolved conflict\n");
    let review = f.rpc(json!({"method":"conflict_detail", "path":"file"}));
    f.rpc(json!({"method":"stage_conflict", "path":"file", "basis":review["basis"]}));
    f.git(&["commit", "-qm", "resolved"]);
    f.rpc(selected_stash_request(
        "stash_drop",
        &stash_selection(&f, 0),
    ));
    assert!(
        f.rpc(json!({"method":"stash_list"}))["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    for kind in ["sha256", "split", "linked", "reftable"] {
        let options: &[&str] = match kind {
            "sha256" => &["--object-format=sha256"],
            "reftable" => &["--ref-format=reftable"],
            _ => &[],
        };
        let mut f = Fixture::with_options(options);
        f.write("file", "one\ntwo\n");
        f.git(&["add", "--", "file"]);
        f.git(&["commit", "-qm", "base"]);
        if kind == "split" {
            f.git(&["update-index", "--split-index"]);
        }
        if kind == "linked" {
            let sibling = f.root.join("linked-checkout");
            f.git(&["worktree", "add", "-qb", "other", sibling.to_str().unwrap()]);
            f.repo = sibling;
        }
        let link = f.repo.join("link");
        symlink("outside-not-followed", &link).unwrap();
        f.git(&["add", "--", "link"]);
        fs::write(f.repo.join("file"), b"changed\0binary\n").unwrap();
        let saved = f.rpc(json!({"method":"stash_save", "message":"", "desktop":true}));
        assert_eq!(saved["created"], true, "{kind}");
        assert_eq!(
            saved["oid"].as_str().unwrap().len(),
            if kind == "sha256" { 64 } else { 40 },
            "{kind}"
        );
        let mut request = selected_stash_request("stash_apply", &stash_selection(&f, 0));
        request["restore_index"] = json!(true);
        request["pop"] = json!(true);
        f.rpc(request);
        assert_eq!(fs::read(f.repo.join("file")).unwrap(), b"changed\0binary\n");
        assert_eq!(
            fs::read_link(link).unwrap(),
            std::path::Path::new("outside-not-followed")
        );
        assert!(
            f.rpc(json!({"method":"stash_list"}))["entries"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
}

#[cfg(unix)]
#[test]
fn structured_stashes_preserve_raw_paths_and_pin_mutations() {
    use std::os::unix::ffi::OsStrExt;
    let unborn = Fixture::new();
    unborn.write("new", "keep");
    let failed = unborn.request(json!({"method":"stash_save"}), false);
    assert_eq!(failed["created"], false);
    assert_eq!(fs::read(unborn.repo.join("new")).unwrap(), b"keep");

    let f = Fixture::new();
    f.write("file", "one\ntwo\n");
    f.git(&["add", "--", "file"]);
    f.git(&["commit", "-qm", "base"]);
    assert!(
        f.rpc(json!({"method":"stash_list"}))["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.rpc(json!({"method":"stash_save","message":"saved"}))["created"],
        false
    );
    f.write("file", "index\ntwo\n");
    f.git(&["add", "--", "file"]);
    f.write("file", "index\nworking\n");
    let raw_bytes = b"-untracked-\xff\n";
    let raw = f.repo.join(OsStr::from_bytes(raw_bytes));
    fs::write(&raw, b"raw saved\n").unwrap();
    f.write(".gitignore", "ignored\n");
    f.git(&["add", "--", ".gitignore"]);
    f.write("ignored", "ignored data");
    f.git(&["tag", "main"]);
    let saved =
        f.rpc(json!({"method":"stash_save","message":"","desktop":true,"include_untracked":true}));
    assert_eq!(saved["created"], true);
    let selected = stash_selection(&f, 0);
    assert_eq!(selected["oid"], saved["oid"]);
    let listing = f.rpc(json!({"method":"stash_list"}));
    let entry = &listing["entries"][0];
    assert_eq!(entry["branch"], "main");
    assert!(
        entry["message"]
            .as_str()
            .unwrap()
            .contains("!!GitHub_Desktop<main>")
    );
    assert_eq!(entry["parents"].as_array().unwrap().len(), 3);
    assert_eq!(fs::read(f.repo.join("file")).unwrap(), b"one\ntwo\n");
    assert!(!raw.exists());
    assert_eq!(fs::read(f.repo.join("ignored")).unwrap(), b"ignored data");
    let capture = f.rpc(json!({"method":"stash_files","oid":entry["oid"]}));
    let files = capture["files"].as_array().unwrap();
    let raw_file = files
        .iter()
        .find(|item| item["path"]["bytes"] == json!(raw_bytes))
        .unwrap();
    let tracked = files
        .iter()
        .find(|item| item["path"]["bytes"] == json!(b"file"))
        .unwrap();
    assert_eq!(raw_file["untracked"], true);
    assert_eq!(tracked["untracked"], false);
    let staged = capture["staged_files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["display_path"] == "file")
        .unwrap();
    for item in [tracked, raw_file, staged] {
        let view = f.rpc(
            json!({"method":"commit_diff","path":item["path"],"commit":item["commit"],"parent":0}),
        );
        assert_eq!(view["can_select"], false);
        assert!(!view["hunks"].as_array().unwrap().is_empty());
    }
    let before = f.git(&["status", "--porcelain=v2", "-z"]).stdout;
    f.rpc(json!({"method":"stash_list"}));
    f.rpc(json!({"method":"stash_files","oid":entry["oid"]}));
    assert_eq!(f.git(&["status", "--porcelain=v2", "-z"]).stdout, before);
    f.write("file", "newer stash\n");
    f.rpc(json!({"method":"stash_save","message":"newer"}));
    for method in ["stash_apply", "stash_drop", "stash_move"] {
        let mut request = selected_stash_request(method, &selected);
        if method == "stash_move" {
            request["branch"] = json!("other");
        }
        f.fails(request);
    }
    assert_eq!(
        f.rpc(json!({"method":"stash_list"}))["entries"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(fs::read(f.repo.join("file")).unwrap(), b"one\ntwo\n");
    let applied = f.rpc(json!({"method":"stash_apply","oid":saved["oid"],"restore_index":true}));
    assert_eq!(applied["retained"], true);
    assert_eq!(fs::read(f.repo.join("file")).unwrap(), b"index\nworking\n");
    assert_eq!(fs::read(&raw).unwrap(), b"raw saved\n");
    assert_eq!(f.git(&["show", ":file"]).stdout, b"index\ntwo\n");
    f.git(&["reset", "--hard", "-q"]);
    fs::remove_file(&raw).unwrap();
    fs::remove_file(f.repo.join("ignored")).unwrap();
    f.git(&["clean", "-fdq"]);
    let mut request = selected_stash_request("stash_apply", &stash_selection(&f, 0));
    request["pop"] = json!(true);
    assert_eq!(f.rpc(request)["retained"], false);
    assert_eq!(fs::read(f.repo.join("file")).unwrap(), b"newer stash\n");
    assert_eq!(
        f.rpc(json!({"method":"stash_list"}))["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    f.git(&["reset", "--hard", "-q"]);
    let original = f.rpc(json!({"method":"stash_list"}))["entries"][0].clone();
    let mut request = selected_stash_request("stash_move", &stash_selection(&f, 0));
    request["branch"] = json!("feature/other");
    assert_eq!(f.rpc(request)["original_retained"], false);
    let after = f.rpc(json!({"method":"stash_list"}))["entries"][0].clone();
    assert_eq!(after["branch"], "feature/other");
    assert_eq!(after["parents"], original["parents"]);
    assert_eq!(after["tree"], original["tree"]);
    let mut request = selected_stash_request("stash_move", &stash_selection(&f, 0));
    request["branch"] = json!("feature/other");
    let unchanged = f.rpc(request);
    assert_eq!(unchanged["unchanged"], true);
    assert_eq!(unchanged["oid"], after["oid"]);
    assert_eq!(
        f.rpc(json!({"method":"stash_list"}))["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        f.rpc(json!({"method":"stash_files","oid":after["oid"]}))["files"]
            .as_array()
            .unwrap()
            .len(),
        files.len()
    );
    f.git(&["config", "user.useConfigOnly", "true"]);
    f.git(&["config", "--unset", "user.name"]);
    f.git(&["config", "--unset", "user.email"]);
    let selected = stash_selection(&f, 0);
    let mut request = selected_stash_request("stash_move", &selected);
    request["branch"] = json!("another");
    f.fails(request);
    assert_eq!(
        f.rpc(json!({"method":"stash_list"}))["entries"][0]["oid"],
        selected["oid"]
    );
    f.git(&["config", "user.name", "Stash Test"]);
    f.git(&["config", "user.email", "stash@example.invalid"]);
    f.rpc(selected_stash_request(
        "stash_drop",
        &stash_selection(&f, 0),
    ));
    assert!(
        f.rpc(json!({"method":"stash_list"}))["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn checkout_rejects_stale_reviews_and_selects_occupied_worktrees() {
    let f = checkout_fixture(true);
    let file = f.repo.join("file");
    fs::write(&file, b"change a\n").unwrap();
    let plan = f.rpc(json!({"method":"checkout_preview", "branch":"other"}));
    let metadata = fs::metadata(&file).unwrap();
    fs::write(&file, b"change b\n").unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&file)
        .unwrap()
        .set_times(
            fs::FileTimes::new()
                .set_accessed(metadata.accessed().unwrap())
                .set_modified(metadata.modified().unwrap()),
        )
        .unwrap();
    f.fails(json!({"method":"checkout", "branch":"other", "strategy":"MoveToNewBranch", "basis":plan["basis"]}));
    assert!(
        f.rpc(json!({"method":"stash_list"}))["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(fs::read(&file).unwrap(), b"change b\n");
    let plan = f.rpc(json!({"method":"checkout_preview", "branch":"other"}));
    f.git(&["config", "filter.example.clean", "cat"]);
    f.fails(json!({"method":"checkout", "branch":"other", "strategy":"MoveToNewBranch", "basis":plan["basis"]}));
    let plan = f.rpc(json!({"method":"checkout_preview", "branch":"other"}));
    let target = f.git_text(&["commit-tree", "other^{tree}", "-m", "changed target"]);
    f.git(&["update-ref", "refs/heads/other", &target]);
    f.fails(json!({"method":"checkout", "branch":"other", "strategy":"MoveToNewBranch", "basis":plan["basis"]}));
    let plan = f.rpc(json!({"method":"checkout_preview", "branch":"other"}));
    f.git(&["branch", "same-head"]);
    f.git(&["symbolic-ref", "HEAD", "refs/heads/same-head"]);
    f.fails(json!({"method":"checkout", "branch":"other", "strategy":"MoveToNewBranch", "basis":plan["basis"]}));
    assert!(!f.repo.join(".git/desktop-checkout").exists());

    let f = checkout_fixture(true);
    f.git(&[
        "remote",
        "add",
        "origin",
        f.root.join("not-contacted").to_str().unwrap(),
    ]);
    let tip = f.git_text(&["rev-parse", "other"]);
    f.git(&["update-ref", "refs/remotes/origin/topic", &tip]);
    let plan = f.rpc(json!({"method":"checkout_preview", "branch":"refs/remotes/origin/topic"}));
    assert_eq!(plan["target"]["remote"], true);
    assert_eq!(plan["target"]["branch"], "topic");
    reviewed_checkout(
        &f,
        "refs/remotes/origin/topic",
        "MoveToNewBranch",
        json!({}),
        true,
    );
    assert_eq!(f.git_text(&["rev-parse", "HEAD"]), tip);
    assert_eq!(f.git_text(&["config", "branch.topic.remote"]), "origin");
    assert_eq!(
        f.git_text(&["config", "branch.topic.merge"]),
        "refs/heads/topic"
    );

    let f = checkout_fixture(true);
    let held = f.root.join("held worktree");
    f.git(&["worktree", "add", "-q", held.to_str().unwrap(), "other"]);
    f.write("file", "keep source");
    let plan = f.rpc(json!({"method":"checkout_preview", "branch":"other"}));
    assert_eq!(
        plan["occupied_worktree"]["display_path"],
        held.to_str().unwrap()
    );
    let blocked = f.request(
        json!({"method":"checkout", "branch":"other", "strategy":"StashOnCurrentBranch"}),
        false,
    );
    assert_eq!(blocked["requires_worktree"], true);
    assert!(
        f.rpc(json!({"method":"stash_list"}))["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let registry = f.root.join("registry.json");
    let result = f
        .command(env!("CARGO_BIN_EXE_desktop-rs"))
        .arg("-C")
        .arg(&f.repo)
        .arg("--config")
        .arg(&registry)
        .args(["checkout", "other", "--open-worktree"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains(held.to_str().unwrap()));
    let saved: Value = serde_json::from_slice(&fs::read(registry).unwrap()).unwrap();
    assert_eq!(saved["selected"], held.to_str().unwrap());
    assert_eq!(fs::read(f.repo.join("file")).unwrap(), b"keep source");
}

#[cfg(unix)]
#[test]
fn checkout_leave_requires_consent_and_restores_index_and_raw_paths() {
    use std::os::unix::ffi::OsStrExt;
    let f = checkout_fixture(true);
    assert_eq!(f.rpc(json!({"method":"checkout_history"})), json!([]));
    assert!(!f.repo.join(".git/desktop-checkout").exists());
    f.git(&["switch", "-q", "other"]);
    f.write("file", "saved destination\n");
    let destination = f.rpc(json!({"method":"stash_save", "desktop":true}))["oid"]
        .as_str()
        .unwrap()
        .to_owned();
    f.git(&["switch", "-q", "main"]);
    f.write("file", "previous saved\n");
    let previous = f.rpc(json!({"method":"stash_save", "desktop":true}))["oid"]
        .as_str()
        .unwrap()
        .to_owned();
    f.write("file", "new index\n");
    f.git(&["add", "--", "file"]);
    f.write("file", "new working\n");
    let raw = f.repo.join(OsStr::from_bytes(b"-raw-\xff\n"));
    fs::write(&raw, b"untracked\n").unwrap();
    let plan = f.rpc(json!({"method":"checkout_preview", "branch":"other"}));
    assert_eq!(plan["dirty"], true);
    assert_eq!(plan["source_stash"]["oid"], previous);
    assert_eq!(plan["target_stash"]["oid"], destination);
    assert_eq!(
        f.request(json!({"method":"checkout", "branch":"other"}), false)["requires_confirmation"],
        true
    );
    assert_eq!(f.request(json!({"method":"checkout", "branch":"other", "strategy":"StashOnCurrentBranch", "basis":plan["basis"]}), false)["requires_confirmation"], true);
    assert_eq!(fs::read(f.repo.join("file")).unwrap(), b"new working\n");
    let value = reviewed_checkout(
        &f,
        "other",
        "StashOnCurrentBranch",
        json!({"replace_stash":true}),
        true,
    );
    let saved = value["saved_oid"].as_str().unwrap();
    assert_eq!(f.git_text(&["symbolic-ref", "--short", "HEAD"]), "other");
    assert_eq!(fs::read(f.repo.join("file")).unwrap(), b"target\n");
    assert!(!raw.exists());
    assert!(f.git(&["status", "--porcelain"]).stdout.is_empty());
    let list = f.rpc(json!({"method":"stash_list"}));
    let entries = list["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().any(|item| item["oid"] == destination));
    assert!(entries.iter().any(|item| item["oid"] == saved));
    assert_eq!(
        f.git(&["show", &format!("{saved}:file")]).stdout,
        b"new working\n"
    );
    let history = f.rpc(json!({"method":"checkout_history"}));
    assert_eq!(history[0]["receipt"]["phase"], "complete");
    assert_eq!(history[0]["receipt"]["replaced_oid"], previous);
    assert_eq!(history[0]["saved_stashes"][0]["oid"], saved);
    reviewed_checkout(&f, "main", "MoveToNewBranch", json!({}), true);
    assert_eq!(fs::read(f.repo.join("file")).unwrap(), b"base\n");
    let list = f.rpc(json!({"method":"stash_list"}));
    assert!(
        list["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["oid"] == saved)
    );
    f.rpc(json!({"method":"stash_apply", "oid":saved, "restore_index":true, "pop":true}));
    assert_eq!(fs::read(f.repo.join("file")).unwrap(), b"new working\n");
    assert_eq!(f.git(&["show", ":file"]).stdout, b"new index\n");
    assert_eq!(fs::read(raw).unwrap(), b"untracked\n");
}

#[test]
fn checkout_bring_preserves_staged_changes_and_retains_conflict_recovery() {
    let f = checkout_fixture(true);
    f.write("new", "new staged");
    f.git(&["add", "--", "new"]);
    f.write("untracked", "keep");
    let result = reviewed_checkout(&f, "other", "MoveToNewBranch", json!({}), true);
    assert!(result["saved_oid"].is_null());
    assert!(
        f.rpc(json!({"method":"stash_list"}))["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(f.git(&["show", ":new"]).stdout, b"new staged");
    assert_eq!(fs::read(f.repo.join("untracked")).unwrap(), b"keep");

    let f = checkout_fixture(false);
    let base: String = (0..30).map(|n| format!("line {n}\n")).collect();
    f.write("file", &base);
    f.git(&["add", "--", "file"]);
    f.git(&["commit", "-qm", "base changes"]);
    f.git(&["switch", "-q", "other"]);
    f.write("file", &base.replace("line 29\n", "target 29\n"));
    f.git(&["add", "--", "file"]);
    f.git(&["commit", "-qm", "target changes"]);
    f.git(&["switch", "-q", "main"]);
    let staged = base.replace("line 0\n", "staged 0\n");
    let working = staged.replace("line 14\n", "working 14\n");
    f.write("file", &staged);
    f.git(&["add", "--", "file"]);
    f.write("file", &working);
    f.write("new", "untracked");
    let result = reviewed_checkout(
        &f,
        "other",
        "MoveToNewBranch",
        json!({"restore_index":true}),
        true,
    );
    assert!(
        result["saved_oid"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
    assert!(
        f.rpc(json!({"method":"stash_list"}))["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fs::read_to_string(f.repo.join("file")).unwrap(),
        working.replace("line 29\n", "target 29\n")
    );
    assert_eq!(
        String::from_utf8(f.git(&["show", ":file"]).stdout).unwrap(),
        staged.replace("line 29\n", "target 29\n")
    );
    assert_eq!(fs::read(f.repo.join("new")).unwrap(), b"untracked");

    let f = checkout_fixture(true);
    f.write("file", "local competing\n");
    reviewed_checkout(&f, "other", "MoveToNewBranch", json!({}), false);
    let history = f.rpc(json!({"method":"checkout_history"}));
    let item = &history[0];
    assert_eq!(item["receipt"]["phase"], "applying");
    assert!(!item["receipt"]["error"].is_null());
    let stash = item["saved_stashes"][0]["oid"].as_str().unwrap();
    assert_eq!(f.git_text(&["symbolic-ref", "--short", "HEAD"]), "other");
    assert!(!f.git(&["ls-files", "-u"]).stdout.is_empty());
    assert_eq!(
        f.git(&["show", &format!("{stash}:file")]).stdout,
        b"local competing\n"
    );
    let before = f.git(&["ls-files", "--stage"]).stdout;
    f.rpc(json!({"method":"acknowledge_checkout", "receipt_id":item["receipt"]["id"]}));
    assert_eq!(f.rpc(json!({"method":"checkout_history"})), json!([]));
    assert_eq!(f.git(&["ls-files", "--stage"]).stdout, before);
    assert_eq!(
        f.rpc(json!({"method":"stash_list"}))["entries"][0]["oid"],
        stash
    );
}

#[test]
fn checkout_previews_reject_stale_inputs_and_handle_worktrees_and_remote_refs() {
    let f = Fixture::new();
    f.write("file", "base\n");
    f.git(&["add", "--", "file"]);
    f.git(&["commit", "-qm", "base"]);
    f.git(&["branch", "other"]);
    let held = f.root.join("missing-worktree");
    f.git(&["worktree", "add", "-q", held.to_str().unwrap(), "other"]);
    fs::remove_dir_all(&held).unwrap();
    f.write("file", "keep local work");
    let plan = f.rpc(json!({"method":"checkout_preview", "branch":"other"}));
    assert!(
        plan["occupied_worktree"]["prunable"]
            .as_str()
            .is_some_and(|reason| !reason.is_empty())
    );
    f.rpc(json!({"method":"worktree_remove", "path":held, "force":false}));
    let plan = f.rpc(json!({"method":"checkout_preview", "branch":"other"}));
    assert!(plan["occupied_worktree"].is_null());
    f.rpc(json!({"method":"checkout", "branch":"other", "strategy":"MoveToNewBranch", "basis":plan["basis"]}));
    assert_eq!(fs::read(f.repo.join("file")).unwrap(), b"keep local work");
    assert_eq!(f.git_text(&["symbolic-ref", "--short", "HEAD"]), "other");

    let f = Fixture::new();
    f.write("file", "base\n");
    f.git(&["add", "--", "file"]);
    f.git(&["commit", "-qm", "base"]);
    f.git(&["branch", "other"]);
    f.write("file", "keep review data");
    let registry = f.root.join("registry.json");
    let preferences = f.root.join("registry.preferences.json");
    fs::write(&preferences, "{broken").unwrap();
    let run_preview = |extra: &[&str]| {
        f.command(env!("CARGO_BIN_EXE_desktop-rs"))
            .arg("-C")
            .arg(&f.repo)
            .arg("--config")
            .arg(&registry)
            .args(["checkout", "other", "--preview"])
            .args(extra)
            .output()
            .unwrap()
    };
    let preview = run_preview(&[]);
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&preview.stdout).unwrap()["dirty"],
        true
    );
    assert_eq!(fs::read(&preferences).unwrap(), b"{broken");
    assert!(!registry.exists() && !f.repo.join(".git/desktop-checkout").exists());
    assert!(!run_preview(&["--open-worktree"]).status.success());
    assert!(!registry.exists());
    let plan = f.rpc(json!({"method":"checkout_preview", "branch":"other"}));
    f.git(&[
        "config",
        "core.hooksPath",
        f.root.join("hooks").to_str().unwrap(),
    ]);
    f.fails(json!({"method":"checkout", "branch":"other", "strategy":"MoveToNewBranch", "basis":plan["basis"]}));
    assert!(
        f.rpc(json!({"method":"stash_list"}))["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(fs::read(f.repo.join("file")).unwrap(), b"keep review data");
    let plan = f.rpc(json!({"method":"checkout_preview", "branch":"other"}));
    fs::write(f.repo.join(".git/info/attributes"), "file review-flag\n").unwrap();
    f.fails(json!({"method":"checkout", "branch":"other", "strategy":"MoveToNewBranch", "basis":plan["basis"]}));
    assert!(
        f.rpc(json!({"method":"stash_list"}))["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(fs::read(f.repo.join("file")).unwrap(), b"keep review data");

    let f = Fixture::new();
    f.write("file", "base\n");
    f.git(&["add", "--", "file"]);
    f.git(&["commit", "-qm", "base"]);
    f.git(&["branch", "other"]);
    f.git(&[
        "remote",
        "add",
        "origin",
        f.root.join("not-contacted").to_str().unwrap(),
    ]);
    let head = f.git_text(&["rev-parse", "HEAD"]);
    f.git(&["update-ref", "refs/remotes/origin/topic", &head]);
    f.git(&["branch", "refs/remotes/origin/topic"]);
    let plan = f.rpc(json!({"method":"checkout_preview", "branch":"refs/remotes/origin/topic"}));
    assert_eq!(plan["target"]["remote"], true);
    assert_eq!(plan["target"]["branch"], "topic");
    f.git(&[
        "update-ref",
        "refs/remotes/origin/refs/heads/missing",
        &head,
    ]);
    f.fails(json!({"method":"checkout_preview", "branch":"refs/heads/missing"}));
    let plan = f.rpc(json!({"method":"checkout_preview", "branch":"refs/remotes/origin/topic"}));
    f.rpc(json!({"method":"checkout", "branch":"refs/remotes/origin/topic", "strategy":"MoveToNewBranch", "basis":plan["basis"]}));
    assert_eq!(f.git_text(&["symbolic-ref", "--short", "HEAD"]), "topic");
    assert_eq!(f.git_text(&["config", "branch.topic.remote"]), "origin");
    assert_eq!(
        f.git_text(&["config", "branch.topic.merge"]),
        "refs/heads/topic"
    );
}

#[test]
fn checkout_recurses_into_submodules_and_preserves_local_work() {
    let child = Fixture::new();
    child.write("file", "base\n");
    child.git(&["add", "--", "file"]);
    child.git(&["commit", "-qm", "base"]);
    let first = child.git_text(&["rev-parse", "HEAD"]);
    child.write("file", "target child\n");
    child.git(&["add", "--", "file"]);
    child.git(&["commit", "-qm", "target"]);
    let second = child.git_text(&["rev-parse", "HEAD"]);

    let parent = Fixture::new();
    parent.write("file", "base\n");
    parent.git(&["add", "--", "file"]);
    parent.git(&["commit", "-qm", "base"]);
    parent.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "--",
        child.repo.to_str().unwrap(),
        "module",
    ]);
    let nested = parent.repo.join("module");
    let nested_git = |args: &[&str]| {
        let result = parent
            .command("git")
            .arg("-C")
            .arg(&nested)
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout).unwrap().trim().to_owned()
    };
    nested_git(&["config", "user.name", "Migration Test"]);
    nested_git(&["config", "user.email", "migration@example.invalid"]);
    nested_git(&["checkout", "--detach", "-q", &first]);
    parent.git(&["add", "--all"]);
    parent.git(&["commit", "-qm", "first submodule"]);
    parent.git(&["switch", "-qc", "other"]);
    nested_git(&["checkout", "--detach", "-q", &second]);
    parent.git(&["add", "--all"]);
    parent.git(&["commit", "-qm", "second submodule"]);
    parent.git(&["switch", "-q", "main"]);
    nested_git(&["checkout", "--detach", "-q", &first]);
    let checkout = |branch: &str, success: bool| {
        let plan = parent.rpc(json!({"method":"checkout_preview", "branch":branch}));
        parent.request(json!({"method":"checkout", "branch":branch, "strategy":"MoveToNewBranch", "basis":plan["basis"]}), success)
    };
    checkout("other", true);
    assert_eq!(nested_git(&["rev-parse", "HEAD"]), second);
    checkout("main", true);
    assert_eq!(nested_git(&["rev-parse", "HEAD"]), first);

    let local = nested.join("local-only");
    fs::write(&local, b"keep one").unwrap();
    let plan = parent.rpc(json!({"method":"checkout_preview", "branch":"other"}));
    let meta = fs::metadata(&local).unwrap();
    fs::write(&local, b"keep two").unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&local)
        .unwrap()
        .set_times(
            fs::FileTimes::new()
                .set_accessed(meta.accessed().unwrap())
                .set_modified(meta.modified().unwrap()),
        )
        .unwrap();
    parent.fails(json!({"method":"checkout", "branch":"other", "strategy":"MoveToNewBranch", "basis":plan["basis"]}));
    assert_eq!(nested_git(&["rev-parse", "HEAD"]), first);
    assert_eq!(fs::read(&local).unwrap(), b"keep two");
    checkout("other", true);
    assert_eq!(nested_git(&["rev-parse", "HEAD"]), second);
    assert_eq!(fs::read(&local).unwrap(), b"keep two");
    checkout("main", true);
    fs::write(nested.join("file"), b"keep competing local\n").unwrap();
    checkout("other", false);
    let history = parent.rpc(json!({"method":"checkout_history"}));
    assert_eq!(history[0]["receipt"]["phase"], "submodules");
    assert!(!history[0]["receipt"]["error"].is_null());
    assert_eq!(
        parent.git_text(&["symbolic-ref", "--short", "HEAD"]),
        "other"
    );
    assert_eq!(nested_git(&["rev-parse", "HEAD"]), first);
    assert_eq!(
        fs::read(nested.join("file")).unwrap(),
        b"keep competing local\n"
    );
    assert_eq!(fs::read(&local).unwrap(), b"keep two");
    nested_git(&[
        "stash",
        "push",
        "--include-untracked",
        "-m",
        "retained child work",
    ]);
    parent.rpc(json!({"method":"submodule_update", "paths":["module"], "recursive":true}));
    assert_eq!(nested_git(&["rev-parse", "HEAD"]), second);
    assert!(nested_git(&["stash", "list"]).contains("retained child work"));
}

#[test]
fn typed_mutations_and_recovery_with_real_git() {
    let f = Fixture::new();
    f.write("file.txt", "initial\n");
    f.stage("file.txt");
    f.write("file.txt", "initial modified\n");
    f.rpc(json!({"method":"unstage", "paths":["file.txt"]}));
    let snapshot = f.rpc(json!({"method":"snapshot"}));
    assert_eq!(snapshot["branch"], "main");
    assert!(snapshot["head"].is_null());
    assert_eq!(snapshot["files"][0]["index"], "?");
    f.stage("file.txt");
    f.rpc(json!({"method":"commit", "summary":"First commit", "description":"Description\nsecond line"}));
    let first = f.git_text(&["rev-parse", "HEAD"]);
    assert!(
        f.git_text(&["log", "-1", "--format=%B"])
            .contains("Description\nsecond line")
    );

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let names: &[&[u8]] = &[b":(glob)*.txt", b"line\nbreak", b"invalid-\xff", b"-flag"];
        for name in names {
            fs::write(f.repo.join(OsStr::from_bytes(name)), b"unusual\n").unwrap();
        }
        for name in names {
            f.rpc(json!({"method":"stage", "paths":[{"bytes":name}]}));
        }
        let staged = f.git(&["diff", "--cached", "--name-only", "-z"]).stdout;
        let staged: Vec<_> = staged.split(|byte| *byte == 0).collect();
        assert!(names.iter().all(|name| staged.contains(name)));
        f.commit("Unusual paths");
    }
    f.fails(json!({"method":"stage", "paths":[]}));
    f.fails(json!({"method":"stage", "paths":["../outside"]}));
    #[cfg(unix)]
    f.fails(json!({"method":"stage", "paths":["/etc/passwd"]}));
    f.fails(json!({"method":"create_branch", "name":"--orphan"}));
    f.fails(json!({"method":"create_branch", "name":"@{-1}"}));
    f.fails(json!({"method":"commit", "summary":"bad\nsummary"}));
    f.fails(json!({"method":"stage", "paths":["file.txt"], "force":true}));

    f.write("file.txt", "second\n");
    f.stage("file.txt");
    assert!(
        f.rpc(json!({"method":"diff", "staged":true, "path":"file.txt"}))["stdout"]
            .as_str()
            .unwrap()
            .contains("+second")
    );
    f.rpc(json!({"method":"unstage", "paths":["file.txt"]}));
    assert_eq!(fs::read(f.repo.join("file.txt")).unwrap(), b"second\n");
    f.rpc(json!({"method":"stash_save", "message":"saved work"}));
    assert_eq!(f.rpc(json!({"method":"snapshot"}))["files"], json!([]));
    let listing = f.rpc(json!({"method":"stash_list"}));
    let entry = &listing["entries"][0];
    assert!(entry["message"].as_str().unwrap().contains("saved work"));
    f.rpc(json!({"method":"stash_apply", "oid":entry["oid"]}));
    assert_eq!(fs::read(f.repo.join("file.txt")).unwrap(), b"second\n");
    f.rpc(json!({"method":"stash_drop", "index":0, "oid":entry["oid"], "basis":listing["basis"]}));
    f.stage("file.txt");
    f.commit("Second");
    let second = f.git_text(&["rev-parse", "HEAD"]);

    f.rpc(json!({"method":"create_branch", "name":"feature@work", "checkout":true}));
    f.write("feature", "feature\n");
    f.stage("feature");
    f.commit("Feature");
    let feature = f.git_text(&["rev-parse", "HEAD"]);
    f.checkout("main");
    f.rpc(json!({"method":"merge", "branch":"feature@work"}));
    assert!(f.repo.join("feature").exists());
    f.rpc(json!({"method":"delete_branch", "name":"feature@work"}));
    f.rpc(json!({"method":"tag_create", "name":"v1", "target":first}));
    assert!(
        f.rpc(json!({"method":"tags"}))["stdout"]
            .as_str()
            .unwrap()
            .contains("v1")
    );
    f.rpc(json!({"method":"tag_delete", "name":"v1"}));

    f.rpc(json!({"method":"create_branch", "name":"pick", "checkout":true}));
    f.git(&["reset", "--hard", &second]);
    f.rpc(json!({"method":"cherry_pick", "commit":feature}));
    let picked = f.git_text(&["rev-parse", "HEAD"]);
    f.rpc(json!({"method":"revert", "commit":picked}));
    assert!(!f.repo.join("feature").exists());
    f.checkout("main");

    f.rpc(json!({"method":"create_branch", "name":"rebased", "checkout":true}));
    f.write("rebase-file", "rebased\n");
    f.stage("rebase-file");
    f.commit("Rebase me");
    f.checkout("main");
    f.write("main-file", "main\n");
    f.stage("main-file");
    f.commit("Advance main");
    f.checkout("rebased");
    f.rpc(json!({"method":"rebase", "branch":"main"}));
    assert!(f.repo.join("main-file").exists() && f.repo.join("rebase-file").exists());
    f.checkout("main");

    let remote = f.root.join("remote.git");
    let output = f
        .command("git")
        .args(["init", "--bare", "-q"])
        .arg(&remote)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    f.rpc(json!({"method":"add_remote", "name":"origin", "url":remote}));
    assert!(
        f.rpc(json!({"method":"remotes"}))["stdout"]
            .as_str()
            .unwrap()
            .contains("origin")
    );
    f.rpc(json!({"method":"push", "remote":"origin", "branch":"main", "set_upstream":true}));
    f.rpc(json!({"method":"fetch", "remote":"origin"}));
    f.rpc(json!({"method":"pull", "remote":"origin", "branch":"main"}));
    f.rpc(json!({"method":"remove_remote", "name":"origin"}));
    assert_eq!(f.rpc(json!({"method":"remotes"}))["stdout"], "");
    f.fails(json!({"method":"fetch", "remote":"--upload-pack=bad"}));
    f.fails(json!({"method":"add_remote", "name":"origin", "url":"ext::malicious"}));

    f.rpc(json!({"method":"create_branch", "name":"conflicting", "checkout":true}));
    f.write("file.txt", "conflict side\n");
    f.stage("file.txt");
    f.commit("Conflict side");
    f.checkout("main");
    f.write("file.txt", "main side\n");
    f.stage("file.txt");
    f.commit("Main side");
    f.fails(json!({"method":"merge", "branch":"conflicting"}));
    assert!(
        f.rpc(json!({"method":"snapshot"}))["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["conflicted"] == true)
    );
    f.rpc(json!({"method":"merge_abort"}));
    assert_eq!(fs::read(f.repo.join("file.txt")).unwrap(), b"main side\n");
    assert_eq!(f.rpc(json!({"method":"snapshot"}))["files"], json!([]));
    f.fails(json!({"method":"rebase", "branch":"conflicting"}));
    assert!(
        f.rpc(json!({"method":"snapshot"}))["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["conflicted"] == true)
    );
    f.rpc(json!({"method":"rebase_abort"}));
    assert_eq!(fs::read(f.repo.join("file.txt")).unwrap(), b"main side\n");
    f.fails(json!({"method":"rebase", "branch":"conflicting"}));
    f.write("file.txt", "resolved\n");
    f.stage("file.txt");
    f.rpc(json!({"method":"rebase_continue"}));
    assert_eq!(f.rpc(json!({"method":"snapshot"}))["files"], json!([]));
    assert_eq!(fs::read(f.repo.join("file.txt")).unwrap(), b"resolved\n");

    let subdir = f.repo.join("subdir");
    fs::create_dir(&subdir).unwrap();
    let output = f
        .command(env!("CARGO_BIN_EXE_desktop-rs"))
        .arg("-C")
        .arg(&subdir)
        .arg("snapshot")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["root"], json!(f.repo));
}
