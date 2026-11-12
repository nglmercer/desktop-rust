//! Durable conflict undo against real Git indexes and working trees.
#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    os::unix::{ffi::OsStringExt, fs::PermissionsExt},
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
            std::env::temp_dir().join(format!("desktop-conflict-undo-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        Self {
            root,
            env: HashMap::new(),
        }
    }
    fn command(&self, program: &str) -> Command {
        let mut cmd = Command::new(program);
        cmd.env("HOME", &self.root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.root.join("global"))
            .env("GIT_TERMINAL_PROMPT", "0")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .envs(&self.env);
        cmd
    }
    fn run<I, S>(&self, program: &str, args: I, input: Option<&[u8]>, fail: bool) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
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
            child.stdin.take().unwrap().write_all(input).unwrap();
        }
        let out = child.wait_with_output().unwrap();
        assert_eq!(
            out.status.success(),
            !fail,
            "stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }
    fn git<I, S>(&self, repo: &Path, args: I, fail: bool) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut argv = vec![OsString::from("-C"), repo.as_os_str().into()];
        argv.extend(args.into_iter().map(|v| v.as_ref().to_owned()));
        self.run("git", argv, None, fail)
    }
    fn text(&self, repo: &Path, args: &[&str]) -> String {
        String::from_utf8(self.git(repo, args, false).stdout)
            .unwrap()
            .trim()
            .to_owned()
    }
    fn init(&self, name: &str, options: &[&str]) -> PathBuf {
        let repo = self.root.join(name);
        let mut args: Vec<OsString> = ["init", "-q", "-b", "main"]
            .into_iter()
            .map(Into::into)
            .collect();
        args.extend(options.iter().map(OsString::from));
        args.push(repo.as_os_str().into());
        self.run("git", args, None, false);
        self.git(&repo, ["config", "user.name", "Conflict Test"], false);
        self.git(
            &repo,
            ["config", "user.email", "conflict@example.invalid"],
            false,
        );
        self.git(&repo, ["config", "commit.gpgsign", "false"], false);
        repo
    }
    fn commit(&self, repo: &Path) {
        self.git(repo, ["add", "--all"], false);
        self.git(repo, ["commit", "-qm", "checkpoint"], false);
    }
    fn rpc_raw(&self, repo: &Path, method: &str, mut body: Value, fail: bool) -> Output {
        body["method"] = json!(method);
        self.run(
            env!("CARGO_BIN_EXE_desktop-rs"),
            [
                OsString::from("-C"),
                repo.as_os_str().into(),
                OsString::from("repo"),
                OsString::from("-"),
            ],
            Some(body.to_string().as_bytes()),
            fail,
        )
    }
    fn rpc(&self, repo: &Path, method: &str, body: Value) -> Value {
        serde_json::from_slice(&self.rpc_raw(repo, method, body, false).stdout).unwrap()
    }
    fn undo(&self, repo: &Path, id: &str, fail: bool) {
        self.rpc_raw(repo, "undo_discard", json!({"undo_id":id}), fail);
    }
    fn resolve(&self, repo: &Path, path: Value, choice: &str) -> Value {
        let review = self.rpc(repo, "conflict_detail", json!({"path":path}));
        self.rpc(
            repo,
            "resolve_conflict",
            json!({"path":path,"choice":choice,"basis":review["basis"]}),
        )
    }
    fn conflict(
        &self,
        name: &str,
        filename: &OsStr,
        attributes: Option<&str>,
        encoding: bool,
        options: &[&str],
        lfs: bool,
    ) -> (PathBuf, PathBuf) {
        let repo = self.init(name, options);
        if lfs {
            assert_eq!(self.rpc(&repo, "lfs_install", json!({}))["code"], 0);
            self.git(&repo, ["lfs", "track", "*.lfs"], false);
        }
        if let Some(attributes) = attributes {
            fs::write(repo.join(".gitattributes"), attributes).unwrap();
        }
        let file = repo.join(filename);
        let content = |s: &str| {
            if encoding {
                s.encode_utf16()
                    .flat_map(u16::to_le_bytes)
                    .collect::<Vec<_>>()
            } else {
                s.as_bytes().to_vec()
            }
        };
        fs::write(&file, content("base\n")).unwrap();
        self.commit(&repo);
        self.git(&repo, ["switch", "-qc", "side"], false);
        fs::write(&file, content("theirs\n")).unwrap();
        self.commit(&repo);
        self.git(&repo, ["switch", "-q", "main"], false);
        fs::write(&file, content("ours\n")).unwrap();
        self.commit(&repo);
        self.git(&repo, ["merge", "side"], true);
        (repo, file)
    }
}
impl Drop for Suite {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn id(value: &Value) -> &str {
    value["undo_id"].as_str().unwrap()
}

#[test]
fn old_journals_choices_and_newer_edits() {
    let s = Suite::new();
    for version in [1, 2] {
        let repo = s.init(&format!("legacy-v{version}"), &[]);
        let file = repo.join("file");
        fs::write(&file, b"base").unwrap();
        s.commit(&repo);
        fs::write(&file, b"edited").unwrap();
        let plan = s.rpc(
            &repo,
            "discard_preview",
            json!({"path":"file", "scope":"all_changes"}),
        );
        let result = s.rpc(
            &repo,
            "discard",
            json!({"path":"file", "scope":"all_changes", "basis":plan["basis"]}),
        );
        let id = result["undo_id"].as_str().unwrap();
        let journal = repo
            .join(".git/desktop-undo")
            .join(id)
            .join("manifest.json");
        let mut manifest: Value = serde_json::from_slice(&fs::read(&journal).unwrap()).unwrap();
        assert_eq!(manifest["version"], 2);
        assert!(manifest.get("purpose").is_none());
        assert!(
            manifest["changes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item.get("skip_working").is_none())
        );
        if version == 1 {
            manifest["version"] = json!(1);
            for key in ["objects", "lfs", "shared", "operation"] {
                manifest.as_object_mut().unwrap().remove(key);
            }
            fs::write(&journal, serde_json::to_vec(&manifest).unwrap()).unwrap();
        }
        s.undo(&repo, id, false);
        assert_eq!(fs::read(file).unwrap(), b"edited");
    }
    for choice in ["ours", "theirs", "delete"] {
        let (repo, file) = s.conflict(choice, OsStr::new("file"), None, false, &[], false);
        let original = fs::read(&file).unwrap();
        let stages = s
            .git(&repo, ["ls-files", "--stage", "--", "file"], false)
            .stdout;
        fs::write(repo.join("unrelated"), b"preserved staged").unwrap();
        s.git(&repo, ["add", "--", "unrelated"], false);
        let unrelated = s
            .git(&repo, ["ls-files", "--stage", "--", "unrelated"], false)
            .stdout;
        let result = s.resolve(&repo, json!("file"), choice);
        let id = id(&result);
        assert_eq!(result["purpose"], "conflict_choice");
        assert!(s.git(&repo, ["ls-files", "-u"], false).stdout.is_empty());
        if let Ok(after) = fs::read(&file) {
            fs::write(&file, b"newer edit").unwrap();
            s.undo(&repo, id, true);
            fs::write(&file, after).unwrap();
        }
        fs::write(repo.join("later"), "later staged").unwrap();
        s.git(&repo, ["add", "--", "later"], false);
        let cli = s.run(
            env!("CARGO_BIN_EXE_desktop-rs"),
            [
                OsString::from("-C"),
                repo.as_os_str().into(),
                OsString::from("undo"),
                OsString::from(id),
            ],
            None,
            false,
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&cli.stdout).unwrap()["code"],
            0
        );
        assert_eq!(fs::read(&file).unwrap(), original);
        assert_eq!(
            s.git(&repo, ["ls-files", "--stage", "--", "file"], false)
                .stdout,
            stages
        );
        assert_eq!(
            s.git(&repo, ["ls-files", "--stage", "--", "unrelated"], false)
                .stdout,
            unrelated
        );
        assert_eq!(
            s.git(&repo, ["show", ":later"], false).stdout,
            b"later staged"
        );
        assert_eq!(
            s.rpc(&repo, "list_discards", json!({}))
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["id"] == id)
                .unwrap()["state"],
            "undone"
        );
    }
    for label in ["edited", "deleted-after-stage"] {
        let (repo, file) = s.conflict(label, OsStr::new("file"), None, false, &[], false);
        let stages = s.git(&repo, ["ls-files", "--stage"], false).stdout;
        fs::write(&file, b"edited resolution\n").unwrap();
        let review = s.rpc(&repo, "conflict_detail", json!({"path":"file"}));
        let result = s.rpc(
            &repo,
            "stage_conflict",
            json!({"path":"file","basis":review["basis"]}),
        );
        assert_eq!(result["purpose"], "conflict_stage");
        if label == "edited" {
            fs::write(&file, b"newer editor contents\n").unwrap();
        } else {
            fs::remove_file(&file).unwrap();
        }
        s.undo(&repo, id(&result), false);
        assert_eq!(s.git(&repo, ["ls-files", "--stage"], false).stdout, stages);
        if label == "edited" {
            assert_eq!(fs::read(file).unwrap(), b"newer editor contents\n");
        } else {
            assert!(!file.exists());
        }
    }
}

#[test]
fn native_paths_encodings_and_prepared_recovery() {
    let s = Suite::new();
    for label in ["raw", "encoded", "sha256", "split", "linked", "reftable"] {
        let filename = if label == "raw" {
            OsString::from_vec(b"-raw-\xff\n".to_vec())
        } else {
            OsString::from("file")
        };
        let options: Vec<&str> = match label {
            "sha256" => vec!["--object-format=sha256"],
            "reftable" => vec!["--ref-format=reftable"],
            _ => vec![],
        };
        let attributes =
            (label == "encoded").then_some("file text working-tree-encoding=UTF-16LE eol=crlf\n");
        let (mut repo, mut file) = s.conflict(
            label,
            &filename,
            attributes,
            label == "encoded",
            &options,
            false,
        );
        if label == "linked" {
            s.git(&repo, ["merge", "--abort"], false);
            let linked = s.root.join("linked-worktree");
            s.git(
                &repo,
                ["worktree", "add", "-qb", "work", linked.to_str().unwrap()],
                false,
            );
            repo = linked;
            file = repo.join(&filename);
            s.git(&repo, ["merge", "side"], true);
        }
        if label == "split" {
            s.git(&repo, ["update-index", "--split-index"], false);
        }
        let before = fs::read(&file).unwrap();
        let stages = s.git(&repo, ["ls-files", "--stage"], false).stdout;
        let path = json!({"bytes":filename.clone().into_vec()});
        let result = s.resolve(&repo, path, "ours");
        s.undo(&repo, id(&result), false);
        assert_eq!(fs::read(&file).unwrap(), before);
        assert_eq!(s.git(&repo, ["ls-files", "--stage"], false).stdout, stages);
    }
    for prefix in ["file-only", "index-only"] {
        let (repo, file) = s.conflict(prefix, OsStr::new("file"), None, false, &[], false);
        let before = fs::read(&file).unwrap();
        let index = fs::read(repo.join(".git/index")).unwrap();
        let result = s.resolve(&repo, json!("file"), "ours");
        let directory = repo.join(".git/desktop-undo").join(id(&result));
        let path = directory.join("manifest.json");
        let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        manifest["state"] = json!("prepared");
        fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        if prefix == "file-only" {
            fs::write(repo.join(".git/index"), index).unwrap();
        } else {
            fs::write(&file, &before).unwrap();
        }
        s.undo(&repo, id(&result), false);
        assert_eq!(fs::read(file).unwrap(), before);
        assert!(!s.git(&repo, ["ls-files", "-u"], false).stdout.is_empty());
    }
    let (repo, _) = s.conflict("stale-context", OsStr::new("file"), None, false, &[], false);
    let result = s.resolve(&repo, json!("file"), "ours");
    let head = s.text(&repo, &["rev-parse", "HEAD"]);
    s.git(&repo, ["branch", "same-head"], false);
    s.git(
        &repo,
        ["symbolic-ref", "HEAD", "refs/heads/same-head"],
        false,
    );
    s.undo(&repo, id(&result), true);
    s.git(&repo, ["symbolic-ref", "HEAD", "refs/heads/main"], false);
    let operation = repo.join(".git/MERGE_MSG");
    let old = fs::read(&operation).unwrap();
    let mut changed = old.clone();
    changed.extend(b"changed");
    fs::write(&operation, changed).unwrap();
    s.undo(&repo, id(&result), true);
    fs::write(&operation, old).unwrap();
    let other = s.text(&repo, &["commit-tree", "HEAD^{tree}", "-m", "other"]);
    s.git(&repo, ["update-ref", "HEAD", &other], false);
    s.undo(&repo, id(&result), true);
    s.git(&repo, ["update-ref", "HEAD", &head], false);
    s.undo(&repo, id(&result), false);
}

#[test]
fn ref_races_symlinks_objects_and_gitlinks() {
    let mut s = Suite::new();
    for changed in ["head", "branch"] {
        let (repo, file) = s.conflict(
            &format!("ref-during-undo-{changed}"),
            OsStr::new("file"),
            None,
            false,
            &["--ref-format=reftable"],
            false,
        );
        let original = s.text(&repo, &["rev-parse", "HEAD"]);
        let new = s.text(&repo, &["commit-tree", "HEAD^{tree}", "-m", "new head"]);
        s.git(&repo, ["branch", "same-head"], false);
        let result = s.resolve(&repo, json!("file"), "ours");
        let published = fs::read(&file).unwrap();
        let index = s.git(&repo, ["ls-files", "--stage"], false).stdout;
        let fake = s.root.join(format!("git-wrapper-{changed}"));
        fs::create_dir(&fake).unwrap();
        let marker = fake.join("change-ref");
        fs::write(&marker, b"change").unwrap();
        let real = s.run("sh", ["-c", "command -v git"], None, false);
        let real = String::from_utf8(real.stdout).unwrap().trim().to_owned();
        let wrapper = fake.join("git");
        let script = "#!/bin/sh\n\"$CONFLICT_REAL_GIT\" \"$@\"\nresult=$?\ncase \" $* \" in\n *\" hash-object \"*\" --no-filters \"*)\n  if [ -e \"$CONFLICT_MARKER\" ]; then\n   rm -f -- \"$CONFLICT_MARKER\"\n   if [ \"$CONFLICT_CHANGE\" = head ]; then\n    \"$CONFLICT_REAL_GIT\" -C \"$CONFLICT_REPO\" update-ref HEAD \"$CONFLICT_NEW_HEAD\" || exit 95\n   else\n    \"$CONFLICT_REAL_GIT\" -C \"$CONFLICT_REPO\" symbolic-ref HEAD refs/heads/same-head || exit 95\n   fi\n  fi;;\nesac\nexit \"$result\"\n";
        fs::write(&wrapper, script).unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
        let path = std::env::join_paths(std::iter::once(fake.clone()).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .unwrap();
        for (key, value) in [
            ("PATH", path),
            ("CONFLICT_REAL_GIT", real.into()),
            ("CONFLICT_MARKER", marker.as_os_str().into()),
            ("CONFLICT_REPO", repo.as_os_str().into()),
            ("CONFLICT_CHANGE", changed.into()),
            ("CONFLICT_NEW_HEAD", new.clone().into()),
        ] {
            s.env.insert(key.into(), value);
        }
        let output = s.rpc_raw(&repo, "undo_discard", json!({"undo_id":id(&result)}), true);
        assert!(String::from_utf8_lossy(&output.stderr).contains("changed before undo"));
        assert!(!marker.exists());
        assert_eq!(fs::read(&file).unwrap(), published);
        assert_eq!(s.git(&repo, ["ls-files", "--stage"], false).stdout, index);
        for key in [
            "PATH",
            "CONFLICT_REAL_GIT",
            "CONFLICT_MARKER",
            "CONFLICT_REPO",
            "CONFLICT_CHANGE",
            "CONFLICT_NEW_HEAD",
        ] {
            s.env.remove(OsStr::new(key));
        }
        s.git(&repo, ["symbolic-ref", "HEAD", "refs/heads/main"], false);
        s.git(&repo, ["update-ref", "HEAD", &original], false);
        s.undo(&repo, id(&result), false);
        assert!(!s.git(&repo, ["ls-files", "-u"], false).stdout.is_empty());
    }
    let repo = s.init("symlink", &[]);
    let link = repo.join("link");
    let target = s.root.join("outside-target");
    fs::write(&target, "keep outside").unwrap();
    std::os::unix::fs::symlink("base-target", &link).unwrap();
    s.commit(&repo);
    s.git(&repo, ["switch", "-qc", "side"], false);
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    s.commit(&repo);
    s.git(&repo, ["switch", "-q", "main"], false);
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink("ours-target", &link).unwrap();
    s.commit(&repo);
    s.git(&repo, ["merge", "side"], true);
    let stages = s.git(&repo, ["ls-files", "--stage"], false).stdout;
    let result = s.resolve(&repo, json!("link"), "theirs");
    assert_eq!(fs::read_link(&link).unwrap(), target);
    s.undo(&repo, id(&result), false);
    assert_eq!(fs::read_link(link).unwrap(), Path::new("ours-target"));
    assert_eq!(fs::read_to_string(target).unwrap(), "keep outside");
    assert_eq!(s.git(&repo, ["ls-files", "--stage"], false).stdout, stages);

    let (repo, file) = s.conflict("objects", OsStr::new("file"), None, false, &[], false);
    let before = fs::read(&file).unwrap();
    let detail = s.rpc(&repo, "conflict_detail", json!({"path":"file"}));
    let oid = detail["stages"][2]["preview"]["oid"].as_str().unwrap();
    fs::write(repo.join("small"), b"replacement").unwrap();
    let replacement = s.text(&repo, &["hash-object", "-w", "--", "small"]);
    s.git(&repo, ["replace", oid, &replacement], false);
    let result = s.resolve(&repo, json!("file"), "ours");
    s.git(&repo, ["replace", "-d", oid], false);
    fs::remove_file(repo.join(".git/objects").join(&oid[..2]).join(&oid[2..])).unwrap();
    s.git(&repo, ["cat-file", "-e", oid], true);
    s.undo(&repo, id(&result), false);
    assert_eq!(
        s.git(&repo, ["cat-file", "blob", oid], false).stdout,
        b"theirs\n"
    );
    assert_eq!(fs::read(file).unwrap(), before);

    let repo = s.init("gitlink", &[]);
    fs::write(repo.join("base"), "base").unwrap();
    s.commit(&repo);
    let child = s.init("gitlink/module", &[]);
    fs::write(child.join("file"), "base").unwrap();
    s.commit(&child);
    let mut ids = vec![s.text(&child, &["rev-parse", "HEAD"])];
    for text in ["ours", "theirs"] {
        fs::write(child.join("file"), text).unwrap();
        s.commit(&child);
        ids.push(s.text(&child, &["rev-parse", "HEAD"]));
    }
    let input: Vec<u8> = ids
        .iter()
        .enumerate()
        .flat_map(|(stage, oid)| format!("160000 {oid} {}\tmodule\0", stage + 1).into_bytes())
        .collect();
    s.run(
        "git",
        [
            OsString::from("-C"),
            repo.as_os_str().into(),
            OsString::from("update-index"),
            OsString::from("-z"),
            OsString::from("--index-info"),
        ],
        Some(&input),
        false,
    );
    let before = s.git(&repo, ["ls-files", "--stage"], false).stdout;
    let result = s.resolve(&repo, json!("module"), "ours");
    fs::write(child.join("file"), "keep local changes").unwrap();
    s.undo(&repo, id(&result), false);
    assert_eq!(s.git(&repo, ["ls-files", "--stage"], false).stdout, before);
    assert_eq!(
        fs::read_to_string(child.join("file")).unwrap(),
        "keep local changes"
    );
}

#[test]
fn conflict_undo_retains_available_lfs_payloads() {
    let mut s = Suite::new();
    let tool = std::env::var_os("GIT_LFS_BIN");
    if let Some(tool) = tool {
        let directory = PathBuf::from(tool).parent().unwrap().to_path_buf();
        let path = std::env::join_paths(std::iter::once(directory).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )))
        .unwrap();
        s.env.insert("PATH".into(), path);
    }
    let status = s
        .command("git")
        .args(["lfs", "version"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    if !status.success() {
        assert!(
            std::env::var_os("DESKTOP_TEST_REQUIRE_LFS").is_none(),
            "Git LFS is required for this test"
        );
        return;
    }
    for label in ["retained-lfs", "missing-lfs", "no-lfs-tool"] {
        let (repo, file) = s.conflict(label, OsStr::new("data.lfs"), None, false, &[], true);
        let before = fs::read(&file).unwrap();
        let stages = s.git(&repo, ["ls-files", "--stage"], false).stdout;
        let detail = s.rpc(&repo, "conflict_detail", json!({"path":"data.lfs"}));
        let caches: Vec<_> = detail["stages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|stage| {
                let oid = stage["preview"]["lfs"]["oid"].as_str().unwrap();
                repo.join(".git/lfs/objects")
                    .join(&oid[..2])
                    .join(&oid[2..4])
                    .join(oid)
            })
            .collect();
        if label == "missing-lfs" {
            fs::remove_file(&caches[2]).unwrap();
        }
        let old_path = s.env.get(OsStr::new("PATH")).cloned();
        if label == "no-lfs-tool" {
            let fake = s.root.join("unavailable-lfs");
            fs::create_dir(&fake).unwrap();
            let tool = fake.join("git-lfs");
            fs::write(&tool, "#!/bin/sh\nexit 1\n").unwrap();
            fs::set_permissions(tool, fs::Permissions::from_mode(0o700)).unwrap();
            let base = old_path
                .clone()
                .or_else(|| std::env::var_os("PATH"))
                .unwrap_or_default();
            let path = std::env::join_paths(
                std::iter::once(fake.clone()).chain(std::env::split_paths(&base)),
            )
            .unwrap();
            s.env.insert("PATH".into(), path);
            s.env.insert("GIT_EXEC_PATH".into(), fake.into());
        }
        let result = s.resolve(
            &repo,
            json!("data.lfs"),
            if label == "no-lfs-tool" {
                "delete"
            } else {
                "ours"
            },
        );
        assert_eq!(
            result["lfs_payloads_unavailable"].as_u64().unwrap() > 0,
            label != "retained-lfs"
        );
        if label == "retained-lfs" {
            for cache in &caches {
                let _ = fs::remove_file(cache);
            }
        }
        s.undo(&repo, id(&result), false);
        assert_eq!(fs::read(file).unwrap(), before);
        assert_eq!(s.git(&repo, ["ls-files", "--stage"], false).stdout, stages);
        if label == "retained-lfs" {
            assert!(caches.iter().all(|cache| cache.exists()));
        }
        if label == "missing-lfs" {
            assert!(!caches[2].exists());
        }
        if let Some(path) = old_path {
            s.env.insert("PATH".into(), path);
        } else {
            s.env.remove(OsStr::new("PATH"));
        }
        s.env.remove(OsStr::new("GIT_EXEC_PATH"));
    }
}

#[test]
fn edited_conflict_review_rejects_stale_context_and_preserves_other_staged_files() {
    use sha2::{Digest, Sha256};
    let s = Suite::new();
    let (repo, file) = s.conflict("stage-review", OsStr::new("file"), None, false, &[], false);
    fs::write(repo.join("unrelated"), b"keep staged").unwrap();
    s.git(&repo, ["add", "--", "unrelated"], false);
    let unrelated = s
        .git(&repo, ["ls-files", "--stage", "--", "unrelated"], false)
        .stdout;
    let review = s.rpc(&repo, "conflict_detail", json!({"path":"file"}));
    assert_eq!(
        review["working"]["sha256"],
        format!("{:x}", Sha256::digest(fs::read(&file).unwrap()))
    );
    let index = fs::read(repo.join(".git/index")).unwrap();
    let blocked = s.rpc_raw(
        &repo,
        "stage_conflict",
        json!({"path":"file","basis":review["basis"]}),
        true,
    );
    let blocked: Value = serde_json::from_slice(&blocked.stdout).unwrap();
    assert_eq!(blocked["requires_confirmation"], true);
    assert!(blocked["markers"].as_u64().unwrap() >= 3);
    assert_eq!(fs::read(repo.join(".git/index")).unwrap(), index);
    fs::write(&file, b"edited result\n").unwrap();
    s.rpc_raw(
        &repo,
        "stage_conflict",
        json!({"path":"file","basis":review["basis"]}),
        true,
    );
    let review = s.rpc(&repo, "conflict_detail", json!({"path":"file"}));
    s.git(&repo, ["config", "core.autocrlf", "true"], false);
    s.rpc_raw(
        &repo,
        "stage_conflict",
        json!({"path":"file","basis":review["basis"]}),
        true,
    );
    s.git(&repo, ["config", "core.autocrlf", "false"], false);
    let review = s.rpc(&repo, "conflict_detail", json!({"path":"file"}));
    s.git(&repo, ["branch", "same-head"], false);
    s.git(
        &repo,
        ["symbolic-ref", "HEAD", "refs/heads/same-head"],
        false,
    );
    s.rpc_raw(
        &repo,
        "stage_conflict",
        json!({"path":"file","basis":review["basis"]}),
        true,
    );
    s.git(&repo, ["symbolic-ref", "HEAD", "refs/heads/main"], false);
    let head = s.text(&repo, &["rev-parse", "HEAD"]);
    let other = s.text(&repo, &["commit-tree", "HEAD^{tree}", "-m", "changed HEAD"]);
    s.git(&repo, ["update-ref", "HEAD", &other], false);
    s.rpc_raw(
        &repo,
        "stage_conflict",
        json!({"path":"file","basis":review["basis"]}),
        true,
    );
    s.git(&repo, ["update-ref", "HEAD", &head], false);
    let message = repo.join(".git/MERGE_MSG");
    let original = fs::read(&message).unwrap();
    let mut changed = original.clone();
    changed.extend(b"changed context");
    fs::write(&message, changed).unwrap();
    s.rpc_raw(
        &repo,
        "stage_conflict",
        json!({"path":"file","basis":review["basis"]}),
        true,
    );
    fs::write(&message, original).unwrap();
    let attrs = repo.join(".gitattributes");
    fs::write(&attrs, "file -text\n").unwrap();
    s.rpc_raw(
        &repo,
        "stage_conflict",
        json!({"path":"file","basis":review["basis"]}),
        true,
    );
    fs::remove_file(attrs).unwrap();
    assert_eq!(fs::read(repo.join(".git/index")).unwrap(), index);
    let review = s.rpc(&repo, "conflict_detail", json!({"path":"file"}));
    assert_eq!(
        s.rpc(
            &repo,
            "stage_conflict",
            json!({"path":"file","basis":review["basis"]})
        )["code"],
        0
    );
    assert_eq!(
        s.git(&repo, ["show", ":file"], false).stdout,
        b"edited result\n"
    );
    assert_eq!(fs::read(file).unwrap(), b"edited result\n");
    assert!(s.git(&repo, ["ls-files", "-u"], false).stdout.is_empty());
    assert_eq!(
        s.git(&repo, ["ls-files", "--stage", "--", "unrelated"], false)
            .stdout,
        unrelated
    );
}

#[test]
fn staging_conflicts_accepts_native_hashes_encodings_raw_paths_and_deletion() {
    let s = Suite::new();
    for kind in ["sha256", "encoded", "split", "raw", "deleted", "markers"] {
        let raw = OsString::from_vec(b"raw-\xff\n".to_vec());
        let filename = if kind == "raw" {
            raw.as_os_str()
        } else {
            OsStr::new("file")
        };
        let attrs =
            (kind == "encoded").then_some("file text working-tree-encoding=UTF-16LE eol=crlf\n");
        let options: &[&str] = if kind == "sha256" {
            &["--object-format=sha256"]
        } else {
            &[]
        };
        let (repo, file) = s.conflict(kind, filename, attrs, kind == "encoded", options, false);
        if kind == "split" {
            s.git(&repo, ["update-index", "--split-index"], false);
        }
        if kind == "deleted" {
            fs::remove_file(&file).unwrap();
        } else if kind != "markers" {
            let bytes = if kind == "encoded" {
                "resolved\r\n"
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes)
                    .collect()
            } else {
                b"resolved\n".to_vec()
            };
            fs::write(&file, bytes).unwrap();
        }
        let path = if kind == "raw" {
            json!({"bytes":b"raw-\xff\n".to_vec()})
        } else {
            json!("file")
        };
        let review = s.rpc(&repo, "conflict_detail", json!({"path":path}));
        if kind == "raw" {
            let cli = s.run(
                env!("CARGO_BIN_EXE_desktop-rs"),
                [
                    OsString::from("-C"),
                    repo.as_os_str().into(),
                    "conflict".into(),
                    raw.clone(),
                ],
                None,
                false,
            );
            let cli: Value = serde_json::from_slice(&cli.stdout).unwrap();
            assert_eq!(cli["basis"], review["basis"]);
        }
        if kind == "deleted" {
            assert_eq!(review["working"]["kind"], "absent");
        }
        let result = s.rpc(
            &repo,
            "stage_conflict",
            json!({"path":path,"basis":review["basis"],"allow_markers":kind == "markers"}),
        );
        assert_eq!(result["code"], 0);
        if kind == "deleted" {
            assert!(
                s.git(&repo, ["ls-files", "--", "file"], false)
                    .stdout
                    .is_empty()
            );
        } else if kind == "markers" {
            assert!(
                s.git(&repo, ["show", ":file"], false)
                    .stdout
                    .windows(7)
                    .any(|v| v == b"<<<<<<<")
            );
        } else if kind != "raw" {
            assert_eq!(s.git(&repo, ["show", ":file"], false).stdout, b"resolved\n");
        } else {
            assert!(s.git(&repo, ["ls-files", "-u"], false).stdout.is_empty());
        }
    }
}

#[test]
fn staging_clean_filter_race_and_cancellation_preserve_index_and_working_file() {
    for kind in ["race", "cancel"] {
        let mut s = Suite::new();
        let (repo, file) = s.conflict(kind, OsStr::new("file"), None, false, &[], false);
        fs::write(repo.join(".gitattributes"), "file filter=slow\n").unwrap();
        let marker = s.root.join("started");
        let release = s.root.join("release");
        let script = s.root.join("clean-filter");
        fs::write(&script, "#!/bin/sh\nprintf started > \"$CLEAN_MARKER\"\nwhile [ ! -e \"$CLEAN_RELEASE\" ]; do sleep 0.02; done\nexec cat\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        s.env
            .insert("CLEAN_MARKER".into(), marker.as_os_str().into());
        s.env
            .insert("CLEAN_RELEASE".into(), release.as_os_str().into());
        s.git(
            &repo,
            ["config", "filter.slow.clean", script.to_str().unwrap()],
            false,
        );
        s.git(&repo, ["config", "filter.slow.required", "true"], false);
        fs::write(&file, b"reviewed before filter\n").unwrap();
        let review = s.rpc(&repo, "conflict_detail", json!({"path":"file"}));
        let index = fs::read(repo.join(".git/index")).unwrap();
        let request =
            json!({"method":"stage_conflict","path":"file","basis":review["basis"]}).to_string();
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
            .write_all(request.as_bytes())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !marker.exists() && Instant::now() < deadline {
            assert!(
                child.try_wait().unwrap().is_none(),
                "staging exited before filter started"
            );
            thread::sleep(Duration::from_millis(20));
        }
        if !marker.exists() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("clean filter did not start");
        }
        if kind == "race" {
            fs::write(&file, b"newer editor changes\n").unwrap();
            fs::write(&release, b"").unwrap();
        } else {
            s.run("kill", ["-INT", &child.id().to_string()], None, false);
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("staging did not exit");
            }
            thread::sleep(Duration::from_millis(20));
        };
        fs::write(&release, b"").unwrap();
        assert!(!status.success());
        if kind == "cancel" {
            assert_eq!(status.code(), Some(130));
        }
        assert_eq!(fs::read(repo.join(".git/index")).unwrap(), index);
        assert_eq!(
            fs::read(file).unwrap(),
            if kind == "race" {
                b"newer editor changes\n".as_slice()
            } else {
                b"reviewed before filter\n".as_slice()
            }
        );
        assert!(!repo.join(".git/index.lock").exists());
        assert!(!fs::read_dir(repo.join(".git")).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("desktop-diff-")
        }));
    }
}

#[test]
fn staging_lfs_conflict_stores_a_pointer_to_edited_content() {
    use sha2::{Digest, Sha256};
    let mut s = Suite::new();
    if let Some(tool) = std::env::var_os("GIT_LFS_BIN") {
        let directory = PathBuf::from(tool).parent().unwrap().to_path_buf();
        let path = std::env::join_paths(std::iter::once(directory).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )))
        .unwrap();
        s.env.insert("PATH".into(), path);
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
            "Git LFS is required for this test"
        );
        return;
    }
    let (repo, file) = s.conflict("stage-lfs", OsStr::new("data.lfs"), None, false, &[], true);
    let data = b"resolved LFS body\n";
    fs::write(&file, data).unwrap();
    let review = s.rpc(&repo, "conflict_detail", json!({"path":"data.lfs"}));
    assert_eq!(
        s.rpc(
            &repo,
            "stage_conflict",
            json!({"path":"data.lfs","basis":review["basis"]})
        )["code"],
        0
    );
    let pointer = String::from_utf8(s.git(&repo, ["show", ":data.lfs"], false).stdout).unwrap();
    assert!(pointer.contains(&format!("oid sha256:{:x}", Sha256::digest(data))));
    assert_eq!(fs::read(file).unwrap(), data);
}
