//! Reviewed whole and partial discards with durable undo.
#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::{
        ffi::OsStringExt,
        fs::{PermissionsExt, symlink},
    },
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
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
        let root = std::env::temp_dir().join(format!("desktop-discard-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let f = Self {
            repo: root.join("repository with spaces"),
            root,
        };
        let mut args: Vec<OsString> = ["init", "-q", "-b", "main"]
            .into_iter()
            .map(Into::into)
            .collect();
        args.extend(options.iter().map(OsString::from));
        args.push(f.repo.as_os_str().into());
        f.run("git", &args, None, false);
        f.git(&["config", "user.name", "Discard Test"]);
        f.git(&["config", "user.email", "discard@example.invalid"]);
        f.git(&["config", "commit.gpgsign", "false"]);
        f
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
            .env_remove("GIT_LITERAL_PATHSPECS");
        cmd
    }
    fn run(&self, program: &str, args: &[OsString], input: Option<&str>, fail: bool) -> Output {
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
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.success(),
            !fail,
            "{program} {args:?}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
    fn git(&self, args: &[&str]) -> Vec<u8> {
        let mut all: Vec<OsString> = vec!["-C".into(), self.repo.as_os_str().into()];
        all.extend(args.iter().map(OsString::from));
        self.run("git", &all, None, false).stdout
    }
    fn rpc_raw(&self, method: &str, mut params: Value, fail: bool) -> Output {
        params["method"] = json!(method);
        self.run(
            env!("CARGO_BIN_EXE_desktop-rs"),
            &[
                "-C".into(),
                self.repo.as_os_str().into(),
                "repo".into(),
                "-".into(),
            ],
            Some(&params.to_string()),
            fail,
        )
    }
    fn rpc(&self, method: &str, params: Value) -> Value {
        serde_json::from_slice(&self.rpc_raw(method, params, false).stdout).unwrap()
    }
    fn preview(&self, path: Value, scope: &str, selection: Value) -> Value {
        self.rpc(
            "discard_preview",
            json!({"path":path,"scope":scope,"selection":selection}),
        )
    }
    fn discard(
        &self,
        path: Value,
        scope: &str,
        selection: Value,
        plan: &Value,
        fail: bool,
    ) -> Output {
        self.rpc_raw(
            "discard",
            json!({"path":path,"scope":scope,"selection":selection,"basis":plan["basis"]}),
            fail,
        )
    }
    fn discard_id(&self, path: Value, scope: &str, selection: Value) -> String {
        let plan = self.preview(path.clone(), scope, selection.clone());
        let output = self.discard(path, scope, selection, &plan, false);
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["undo_id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    fn undo(&self, id: &str, fail: bool) {
        self.rpc_raw("undo_discard", json!({"undo_id":id}), fail);
    }
    fn commit(&self) {
        self.git(&["add", "--all"]);
        self.git(&["commit", "-qm", "checkpoint"]);
    }
    fn indexed(&self, path: &str) -> Vec<u8> {
        self.git(&["show", &format!(":{path}")])
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn file() -> Value {
    json!("file.txt")
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

// The test executable is a byte-preserving Git filter. FD 3 bypasses libtest's
// output while the wrapper keeps its normal status messages off Git's stdout.
#[test]
fn filter_child() {
    let Some(mode) = std::env::var_os("DESKTOP_DISCARD_FILTER") else {
        return;
    };
    let mut bytes = Vec::new();
    std::io::stdin().read_to_end(&mut bytes).unwrap();
    let mut output = OpenOptions::new().write(true).open("/dev/fd/3").unwrap();
    if mode == "clean" {
        output
            .write_all(bytes.strip_prefix(b"W:").unwrap_or(&bytes))
            .unwrap();
    } else {
        output.write_all(b"W:").unwrap();
        output.write_all(&bytes).unwrap();
    }
}

#[test]
fn discard_and_undo_preserve_reviewed_data() {
    let f = Fixture::new();
    let repo = &f.repo;
    let file_path = repo.join("file.txt");
    let baseline: Vec<u8> = (0..70)
        .flat_map(|number| format!("line {number}\n").into_bytes())
        .collect();
    fs::write(&file_path, &baseline).unwrap();
    f.commit();
    let staged = String::from_utf8(baseline.clone())
        .unwrap()
        .replace("line 2\n", "STAGED TWO\n")
        .into_bytes();
    fs::write(&file_path, &staged).unwrap();
    f.git(&["add", "--", "file.txt"]);
    let working = String::from_utf8(staged.clone())
        .unwrap()
        .replace("line 50\n", "WORKING FIFTY\n")
        .into_bytes();
    fs::write(&file_path, &working).unwrap();
    let original_index = fs::read(repo.join(".git/index")).unwrap();
    let plan = f.preview(file(), "working_tree", Value::Null);
    assert_eq!(plan["index_changes"], false);
    let id = f.discard_id(file(), "working_tree", Value::Null);
    let private = repo.join(".git/desktop-undo").join(&id);
    assert_eq!(
        fs::metadata(&private).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(private.join("0.before"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(fs::read(&file_path).unwrap(), staged);
    assert_eq!(f.indexed("file.txt"), staged);
    assert_eq!(fs::read(repo.join(".git/index")).unwrap(), original_index);
    f.undo(&id, false);
    assert_eq!(fs::read(&file_path).unwrap(), working);
    assert_eq!(f.indexed("file.txt"), staged);
    let plan = f.preview(file(), "all_changes", Value::Null);
    assert_eq!(plan["index_changes"], true);
    assert_eq!(plan["backup_bytes"], working.len() + staged.len());
    let result = f.discard(file(), "all_changes", Value::Null, &plan, false);
    let id = serde_json::from_slice::<Value>(&result.stdout).unwrap()["undo_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(fs::read(&file_path).unwrap(), baseline);
    assert_eq!(f.indexed("file.txt"), baseline);
    fs::write(repo.join("unrelated"), "keep staged\n").unwrap();
    f.git(&["add", "--", "unrelated"]);
    let unrelated = f.git(&["ls-files", "--stage", "--", "unrelated"]);
    f.undo(&id, false);
    assert_eq!(fs::read(&file_path).unwrap(), working);
    assert_eq!(f.indexed("file.txt"), staged);
    assert_eq!(
        f.git(&["ls-files", "--stage", "--", "unrelated"]),
        unrelated
    );
    f.undo(&id, true);
    let before = String::from_utf8(staged.clone())
        .unwrap()
        .replace("line 15\n", "working fifteen\n")
        .replace("line 55\n", "working fifty five")
        .into_bytes();
    fs::write(&file_path, &before).unwrap();
    let view = f.rpc("diff_detail", json!({"path":"file.txt"}));
    let lines: Vec<_> = view["hunks"][0]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|line| line["kind"] == "added" || line["kind"] == "deleted")
        .map(|line| line["id"].clone())
        .collect();
    let selection = json!({"basis":view["basis"], "lines":lines});
    let id = f.discard_id(file(), "working_tree", selection);
    assert_eq!(f.indexed("file.txt"), staged);
    assert_eq!(
        fs::read(&file_path).unwrap(),
        String::from_utf8(before.clone())
            .unwrap()
            .replace("working fifteen\n", "line 15\n")
            .into_bytes()
    );
    f.undo(&id, false);
    assert_eq!(fs::read(&file_path).unwrap(), before);
    let plan = f.preview(file(), "all_changes", Value::Null);
    let mut changed = before.clone();
    changed.extend(b"external\n");
    fs::write(&file_path, &changed).unwrap();
    f.discard(file(), "all_changes", Value::Null, &plan, true);
    assert_eq!(fs::read(&file_path).unwrap(), changed);
    fs::write(&file_path, &before).unwrap();
    let plan = f.preview(file(), "all_changes", Value::Null);
    f.git(&["add", "--", "file.txt"]);
    f.discard(file(), "all_changes", Value::Null, &plan, true);
    assert_eq!(fs::read(&file_path).unwrap(), before);
    f.git(&["reset", "--hard", "-q", "HEAD"]);
    f.git(&["clean", "-fdq"]);
    for (name, content) in [
        (b":(glob)*.txt".as_slice(), b"literal\n".as_slice()),
        (b"-option", b""),
        (b"line\nbreak", b"new\n"),
        (b"raw-\xff", b"bytes\n"),
    ] {
        let native = repo.join(OsString::from_vec(name.to_vec()));
        fs::write(&native, content).unwrap();
        let path = json!({"bytes":name});
        let id = f.discard_id(path, "all_changes", Value::Null);
        assert!(!native.exists());
        f.undo(&id, false);
        assert_eq!(fs::read(native).unwrap(), content);
    }
    f.git(&["clean", "-fdq"]);
    let directory = repo.join("nested");
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("deleted"), b"restore parent\n").unwrap();
    f.commit();
    fs::remove_file(directory.join("deleted")).unwrap();
    fs::remove_dir(&directory).unwrap();
    let id = f.discard_id(json!("nested/deleted"), "all_changes", Value::Null);
    assert_eq!(
        fs::read(directory.join("deleted")).unwrap(),
        b"restore parent\n"
    );
    f.undo(&id, false);
    assert!(!directory.join("deleted").exists());
    f.git(&["reset", "--hard", "-q"]);
    f.git(&["mv", "--", "file.txt", "renamed.txt"]);
    let renamed = repo.join("renamed.txt");
    fs::write(&renamed, b"edited destination\n").unwrap();
    let before_rename = f.git(&["ls-files", "--stage"]);
    let plan = f.preview(json!("renamed.txt"), "all_changes", Value::Null);
    assert_eq!(plan["files"].as_array().unwrap().len(), 2);
    let output = f.discard(
        json!("renamed.txt"),
        "all_changes",
        Value::Null,
        &plan,
        false,
    );
    let id = serde_json::from_slice::<Value>(&output.stdout).unwrap()["undo_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(fs::read(&file_path).unwrap(), baseline);
    assert!(!renamed.exists());
    let journal = repo.join(".git/desktop-undo").join(&id);
    let manifest_path = journal.join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["state"] = json!("prepared");
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    fs::write(&renamed, fs::read(journal.join("0.before")).unwrap()).unwrap();
    f.rpc_raw("forget_discard", json!({"undo_id":id}), true);
    assert!(journal.exists());
    f.undo(&id, false);
    assert!(!file_path.exists());
    assert_eq!(fs::read(renamed).unwrap(), b"edited destination\n");
    assert_eq!(f.git(&["ls-files", "--stage"]), before_rename);
    f.git(&["reset", "--hard", "-q"]);
    f.git(&["clean", "-fdq"]);
    fs::write(&file_path, b"valuable\n").unwrap();
    let id = f.discard_id(file(), "all_changes", Value::Null);
    fs::write(&file_path, b"new external edit\n").unwrap();
    f.undo(&id, true);
    assert_eq!(fs::read(&file_path).unwrap(), b"new external edit\n");
    fs::write(&file_path, &baseline).unwrap();
    let backup = repo.join(".git/desktop-undo").join(&id).join("0.before");
    fs::write(&backup, b"tampered").unwrap();
    f.undo(&id, true);
    assert_eq!(fs::read(&file_path).unwrap(), baseline);
    fs::write(&backup, b"valuable\n").unwrap();
    f.undo(&id, false);
    assert_eq!(fs::read(&file_path).unwrap(), b"valuable\n");
    f.git(&["reset", "--hard", "-q"]);
    let large = repo.join("large.bin");
    let old = b"a\0b\xff".repeat(3 * 1024 * 1024);
    fs::write(&large, &old).unwrap();
    fs::set_permissions(&large, fs::Permissions::from_mode(0o750)).unwrap();
    f.commit();
    let new = b"edited\0".repeat(2 * 1024 * 1024);
    fs::write(&large, &new).unwrap();
    let id = f.discard_id(json!("large.bin"), "all_changes", Value::Null);
    assert_eq!(fs::read(&large).unwrap(), old);
    assert_eq!(
        fs::metadata(&large).unwrap().permissions().mode() & 0o777,
        0o750
    );
    f.undo(&id, false);
    assert_eq!(fs::read(&large).unwrap(), new);
    assert_eq!(
        fs::metadata(&large).unwrap().permissions().mode() & 0o777,
        0o750
    );
    f.git(&["reset", "--hard", "-q"]);
    let outside = f.root.join("outside");
    let other = f.root.join("other");
    fs::write(&outside, b"outside remains\n").unwrap();
    fs::write(&other, b"other remains\n").unwrap();
    let link = repo.join("link");
    symlink(&outside, &link).unwrap();
    f.commit();
    fs::remove_file(&link).unwrap();
    symlink(&other, &link).unwrap();
    let id = f.discard_id(json!("link"), "all_changes", Value::Null);
    assert_eq!(fs::read_link(&link).unwrap(), outside);
    f.undo(&id, false);
    assert_eq!(fs::read_link(&link).unwrap(), other);
    assert_eq!(fs::read(outside.clone()).unwrap(), b"outside remains\n");
    assert_eq!(fs::read(other).unwrap(), b"other remains\n");
    f.git(&["reset", "--hard", "-q"]);
    fs::write(repo.join(".gitattributes"), "file.txt text eol=crlf\n").unwrap();
    f.commit();
    fs::write(&file_path, b"new\r\ncontent\r\n").unwrap();
    let id = f.discard_id(file(), "all_changes", Value::Null);
    let restored = fs::read(&file_path).unwrap();
    assert!(restored.windows(2).any(|v| v == b"\r\n"));
    assert!(
        restored
            .split(|v| *v == b'\n')
            .take_while(|v| !v.is_empty())
            .all(|line| line.ends_with(b"\r"))
    );
    f.undo(&id, false);
    assert_eq!(fs::read(&file_path).unwrap(), b"new\r\ncontent\r\n");
    f.git(&["reset", "--hard", "-q"]);
    let wrapper = f.root.join("filter wrapper.sh");
    let executable = std::env::current_exe().unwrap();
    let script = format!(
        "#!/bin/sh\nexport DESKTOP_DISCARD_FILTER=\"$1\"\nexec 3>&1\nexec {} --exact filter_child --nocapture >/dev/null\n",
        quote(executable.to_str().unwrap())
    );
    fs::write(&wrapper, script).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let filter = quote(wrapper.to_str().unwrap());
    f.git(&["config", "filter.wrap.clean", &format!("{filter} clean")]);
    f.git(&["config", "filter.wrap.smudge", &format!("{filter} smudge")]);
    f.git(&["config", "filter.wrap.required", "true"]);
    fs::write(repo.join(".gitattributes"), "file.txt filter=wrap\n").unwrap();
    fs::write(&file_path, b"W:stored\n").unwrap();
    f.commit();
    fs::write(&file_path, b"W:changed\n").unwrap();
    let id = f.discard_id(file(), "all_changes", Value::Null);
    assert_eq!(fs::read(&file_path).unwrap(), b"W:stored\n");
    f.undo(&id, false);
    assert_eq!(fs::read(&file_path).unwrap(), b"W:changed\n");
    f.git(&["config", "filter.wrap.smudge", "false"]);
    let old_index = fs::read(repo.join(".git/index")).unwrap();
    f.rpc_raw("discard_preview", json!({"path":"file.txt"}), true);
    assert_eq!(fs::read(&file_path).unwrap(), b"W:changed\n");
    assert_eq!(fs::read(repo.join(".git/index")).unwrap(), old_index);
    f.git(&["config", "filter.wrap.smudge", &format!("{filter} smudge")]);
    let plan = f.preview(file(), "all_changes", Value::Null);
    for name in ["index.lock", "HEAD.lock"] {
        let lock = repo.join(".git").join(name);
        fs::write(&lock, b"foreign").unwrap();
        f.discard(file(), "all_changes", Value::Null, &plan, true);
        assert_eq!(fs::read(&lock).unwrap(), b"foreign");
        fs::remove_file(lock).unwrap();
    }
    assert!(!repo.join(".git/index.lock").exists());
    let intent = repo.join("intent");
    fs::write(&intent, b"intent to add\n").unwrap();
    f.git(&["add", "-N", "--", "intent"]);
    let before_debug = f.git(&["ls-files", "--debug", "--", "intent"]);
    let plan = f.preview(json!("intent"), "all_changes", Value::Null);
    assert_eq!(plan["undo_requires_unchanged_index"], true);
    let output = f.discard(json!("intent"), "all_changes", Value::Null, &plan, false);
    let id = serde_json::from_slice::<Value>(&output.stdout).unwrap()["undo_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(!intent.exists());
    f.undo(&id, false);
    assert_eq!(fs::read(&intent).unwrap(), b"intent to add\n");
    assert_eq!(
        f.git(&["ls-files", "--debug", "--", "intent"]),
        before_debug
    );
    f.git(&["reset", "--", "intent"]);
    fs::remove_file(intent).unwrap();
    f.rpc("forget_discard", json!({"undo_id":id}));
    assert!(!repo.join(".git/desktop-undo").join(&id).exists());
    assert!(
        f.rpc("list_discards", json!({}))
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["id"] != id)
    );
    let bad_id = "00000000-0000-4000-8000-000000000000";
    let bad = repo.join(".git/desktop-undo").join(bad_id);
    fs::create_dir(&bad).unwrap();
    fs::write(bad.join("manifest.json"), "{broken").unwrap();
    assert!(
        f.rpc("list_discards", json!({}))
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == bad_id && item["state"] == "invalid")
    );
    f.undo(bad_id, true);
    for path in [".git/config", "../outside", outside.to_str().unwrap(), ""] {
        f.rpc_raw("discard_preview", json!({"path":path}), true);
    }
    fn no_change_temporary(path: &Path) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            assert!(!name.to_string_lossy().starts_with(".desktop-change-"));
            if entry.file_type().unwrap().is_dir() {
                no_change_temporary(&entry.path());
            }
        }
    }
    no_change_temporary(repo);
    assert!(fs::read_dir(repo.join(".git")).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".lock")
    }));
    assert!(
        fs::read_dir(repo.join(".git/desktop-undo"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".preparing-"))
    );
}

#[test]
fn unborn_index_and_intent_to_add_can_be_undone() {
    let f = Fixture::new();
    let new = f.repo.join("new");
    fs::write(&new, b"new staged\n").unwrap();
    f.git(&["add", "--", "new"]);
    let id = f.discard_id(json!("new"), "all_changes", Value::Null);
    assert!(!new.exists());
    assert!(f.git(&["ls-files"]).is_empty());
    f.undo(&id, false);
    assert_eq!(fs::read(&new).unwrap(), b"new staged\n");
    assert_eq!(f.indexed("new"), b"new staged\n");
    f.git(&["reset", "--", "new"]);
    fs::remove_file(&new).unwrap();
    let empty = f.repo.join("empty");
    fs::write(&empty, b"").unwrap();
    let id = f.discard_id(json!("empty"), "all_changes", Value::Null);
    f.undo(&id, false);
    assert_eq!(fs::read(&empty).unwrap(), b"");
    fs::write(&new, b"one\ntwo\n").unwrap();
    f.git(&["add", "-N", "--", "new"]);
    let view = f.rpc("diff_detail", json!({"path":"new"}));
    assert!(view["before"]["mode"].is_null());
    let lines: Vec<_> = view["hunks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|h| h["lines"].as_array().unwrap())
        .filter(|line| line["kind"] == "added" || line["kind"] == "deleted")
        .map(|line| line["id"].clone())
        .collect();
    let id = f.discard_id(
        json!("new"),
        "working_tree",
        json!({"basis":view["basis"],"lines":lines}),
    );
    assert!(!new.exists());
    f.undo(&id, false);
    assert_eq!(fs::read(&new).unwrap(), b"one\ntwo\n");
}

#[test]
fn conflict_recovery_and_review_fences_preserve_state() {
    let f = Fixture::new();
    let file = f.repo.join("file");
    fs::write(&file, b"base\n").unwrap();
    f.commit();
    f.git(&["switch", "-qc", "side"]);
    fs::write(&file, b"side\n").unwrap();
    f.commit();
    f.git(&["switch", "-q", "main"]);
    fs::write(&file, b"main\n").unwrap();
    f.commit();
    let merge = f
        .command("git")
        .args(["-C", f.repo.to_str().unwrap(), "merge", "side"])
        .output()
        .unwrap();
    assert!(!merge.status.success());
    let stages = f.git(&["ls-files", "--stage"]);
    let markers = fs::read(&file).unwrap();
    f.rpc_raw(
        "discard_preview",
        json!({"path":"file","scope":"working_tree"}),
        true,
    );
    let id = f.discard_id(json!("file"), "all_changes", Value::Null);
    assert_eq!(fs::read(&file).unwrap(), b"main\n");
    assert!(f.git(&["ls-files", "-u"]).is_empty());
    f.undo(&id, false);
    assert_eq!(fs::read(&file).unwrap(), markers);
    assert_eq!(f.git(&["ls-files", "--stage"]), stages);
    let id = f.discard_id(json!("file"), "all_changes", Value::Null);
    f.git(&["merge", "--abort"]);
    let original = fs::read(&file).unwrap();
    f.undo(&id, true);
    assert_eq!(fs::read(&file).unwrap(), original);
    assert!(f.git(&["ls-files", "-u"]).is_empty());
    fs::write(&file, b"external changed\n").unwrap();
    let plan = f.preview(json!("file"), "all_changes", Value::Null);
    let new =
        String::from_utf8(f.git(&["commit-tree", "HEAD^{tree}", "-m", "other HEAD"])).unwrap();
    let old = String::from_utf8(f.git(&["rev-parse", "HEAD"])).unwrap();
    f.git(&["update-ref", "HEAD", new.trim()]);
    f.discard(json!("file"), "all_changes", Value::Null, &plan, true);
    assert_eq!(fs::read(&file).unwrap(), b"external changed\n");
    f.git(&["update-ref", "HEAD", old.trim()]);
    let plan = f.preview(json!("file"), "all_changes", Value::Null);
    fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
    f.discard(json!("file"), "all_changes", Value::Null, &plan, true);
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o640
    );
}

#[test]
fn flagged_index_restore_requires_unchanged_index() {
    let f = Fixture::new();
    let file = f.repo.join("file");
    fs::write(&file, b"main\n").unwrap();
    f.commit();
    for flag in ["skip-worktree", "assume-unchanged"] {
        fs::write(&file, b"staged flagged\n").unwrap();
        f.git(&["add", "--", "file"]);
        f.git(&["update-index", &format!("--{flag}"), "--", "file"]);
        let before = fs::read(f.repo.join(".git/index")).unwrap();
        let id = f.discard_id(json!("file"), "all_changes", Value::Null);
        fs::write(f.repo.join("unrelated"), b"new\n").unwrap();
        f.git(&["add", "--", "unrelated"]);
        f.undo(&id, true);
        assert_eq!(fs::read(&file).unwrap(), b"main\n");
        let after = fs::read(
            f.repo
                .join(".git/desktop-undo")
                .join(&id)
                .join("index.after"),
        )
        .unwrap();
        fs::write(f.repo.join(".git/index"), after).unwrap();
        f.undo(&id, false);
        assert_eq!(fs::read(&file).unwrap(), b"staged flagged\n");
        assert_eq!(fs::read(f.repo.join(".git/index")).unwrap(), before);
        f.git(&["update-index", &format!("--no-{flag}"), "--", "file"]);
        f.git(&["reset", "--hard", "-q"]);
        f.git(&["clean", "-fdq"]);
    }
}

#[test]
fn selected_utf16_crlf_discard_restores_exact_bytes_without_index_changes() {
    let f = Fixture::new();
    fs::write(
        f.repo.join(".gitattributes"),
        "utf16.txt text working-tree-encoding=UTF-16LE eol=crlf\n",
    )
    .unwrap();
    let encode = |value: &str| {
        value
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>()
    };
    let file = f.repo.join("utf16.txt");
    let original = encode("one\r\ntwo\r\nthree");
    fs::write(&file, &original).unwrap();
    f.commit();
    let modified = encode("ONE\r\ntwo\r\nTHREE");
    fs::write(&file, &modified).unwrap();
    let selection_for = |view: &Value| {
        let lines: Vec<_> = view["hunks"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|h| h["lines"].as_array().unwrap())
            .filter(|line| line["kind"] == "added" || line["kind"] == "deleted")
            .map(|line| line["id"].clone())
            .collect();
        json!({"basis":view["basis"],"lines":lines})
    };
    let view = f.rpc("diff_detail", json!({"path":"utf16.txt"}));
    let mut selection = selection_for(&view);
    let index = fs::read(f.repo.join(".git/index")).unwrap();
    f.git(&["config", "core.eol", "lf"]);
    f.rpc_raw(
        "discard_preview",
        json!({"path":"utf16.txt","scope":"working_tree","selection":selection}),
        true,
    );
    let view = f.rpc("diff_detail", json!({"path":"utf16.txt"}));
    selection["basis"] = view["basis"].clone();
    let id = f.discard_id(json!("utf16.txt"), "working_tree", selection);
    assert_eq!(fs::read(&file).unwrap(), original);
    assert_eq!(fs::read(f.repo.join(".git/index")).unwrap(), index);
    f.undo(&id, false);
    assert_eq!(fs::read(&file).unwrap(), modified);
}

#[test]
fn sha256_detached_and_linked_worktree_recovery_stays_local() {
    let mut f = Fixture::with_options(&["--object-format=sha256"]);
    let file = f.repo.join("file");
    fs::write(&file, b"base\n").unwrap();
    f.commit();
    f.git(&["config", "core.splitIndex", "true"]);
    f.git(&["update-index", "--split-index"]);
    assert!(fs::read_dir(f.repo.join(".git")).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("sharedindex.")
    }));
    f.git(&["switch", "--detach", "-q"]);
    fs::write(&file, b"detached\n").unwrap();
    let id = f.discard_id(json!("file"), "all_changes", Value::Null);
    f.undo(&id, false);
    assert_eq!(fs::read(&file).unwrap(), b"detached\n");
    f.git(&["reset", "--hard", "-q"]);
    f.git(&["switch", "-q", "main"]);
    let sibling = f.root.join("linked tree");
    f.git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "linked",
        sibling.to_str().unwrap(),
    ]);
    let parent = f.repo.clone();
    let index = fs::read(parent.join(".git/index")).unwrap();
    f.repo = sibling.clone();
    fs::write(sibling.join("file"), b"linked\n").unwrap();
    let id = f.discard_id(json!("file"), "all_changes", Value::Null);
    let gitdir = String::from_utf8(f.git(&["rev-parse", "--absolute-git-dir"])).unwrap();
    assert!(
        Path::new(gitdir.trim())
            .join("desktop-undo")
            .join(&id)
            .join("manifest.json")
            .exists()
    );
    f.undo(&id, false);
    assert_eq!(fs::read(sibling.join("file")).unwrap(), b"linked\n");
    assert_eq!(fs::read(parent.join(".git/index")).unwrap(), index);
}

#[test]
fn replaced_parent_symlink_cannot_redirect_undo() {
    let f = Fixture::new();
    fs::create_dir(f.repo.join("dir")).unwrap();
    let file = f.repo.join("dir/file");
    fs::write(&file, b"base\n").unwrap();
    f.commit();
    fs::write(&file, b"edited\n").unwrap();
    let id = f.discard_id(json!("dir/file"), "all_changes", Value::Null);
    fs::rename(f.repo.join("dir"), f.repo.join("saved-dir")).unwrap();
    let outside = f.root.join("outside-dir");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("file"), b"outside\n").unwrap();
    symlink(&outside, f.repo.join("dir")).unwrap();
    f.undo(&id, true);
    assert_eq!(fs::read(outside.join("file")).unwrap(), b"outside\n");
    fs::remove_file(f.repo.join("dir")).unwrap();
    fs::rename(f.repo.join("saved-dir"), f.repo.join("dir")).unwrap();
    f.undo(&id, false);
    assert_eq!(fs::read(file).unwrap(), b"edited\n");
}

#[test]
fn reftable_discard_is_refused_without_mutation() {
    let f = Fixture::with_options(&["--ref-format=reftable"]);
    let file = f.repo.join("file");
    fs::write(&file, b"base").unwrap();
    f.commit();
    fs::write(&file, b"changed").unwrap();
    let plan = f.preview(json!("file"), "all_changes", Value::Null);
    f.discard(json!("file"), "all_changes", Value::Null, &plan, true);
    assert_eq!(fs::read(file).unwrap(), b"changed");
    assert!(!f.repo.join(".git/index.lock").exists());
}

#[test]
fn blocking_smudge_cannot_change_head_and_cancellation_cleans_locks() {
    let f = Fixture::new();
    let file = f.repo.join("file");
    fs::write(&file, b"base\n").unwrap();
    f.commit();
    let alternate =
        String::from_utf8(f.git(&["commit-tree", "HEAD^{tree}", "-m", "alternate"])).unwrap();
    let started = f.root.join("filter-started");
    let block = f.root.join("filter-block");
    let attempted = f.root.join("filter-attempted");
    let script = f.root.join("guard.sh");
    let source = format!(
        "#!/bin/sh\nif [ -e {} ]; then\n git -C {} update-ref HEAD {} >/dev/null 2>&1\n printf '%s' \"$?\" > {}\nfi\nif [ -e {} ]; then\n printf started > {}\n sleep 60\nfi\nexec cat\n",
        quote(f.repo.join(".git/HEAD.lock").to_str().unwrap()),
        quote(f.repo.to_str().unwrap()),
        quote(alternate.trim()),
        quote(attempted.to_str().unwrap()),
        quote(block.to_str().unwrap()),
        quote(started.to_str().unwrap())
    );
    fs::write(&script, source).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    f.git(&["config", "filter.guard.clean", "cat"]);
    f.git(&[
        "config",
        "filter.guard.smudge",
        &quote(script.to_str().unwrap()),
    ]);
    f.git(&["config", "filter.guard.required", "true"]);
    fs::write(f.repo.join(".gitattributes"), "file filter=guard\n").unwrap();
    f.commit();
    let head = f.git(&["rev-parse", "HEAD"]);
    fs::write(&file, b"edited\n").unwrap();
    let id = f.discard_id(json!("file"), "all_changes", Value::Null);
    assert_ne!(fs::read_to_string(&attempted).unwrap(), "0");
    assert_eq!(f.git(&["rev-parse", "HEAD"]), head);
    f.undo(&id, false);
    assert_eq!(fs::read(&file).unwrap(), b"edited\n");
    let plan = f.preview(json!("file"), "all_changes", Value::Null);
    let index = fs::read(f.repo.join(".git/index")).unwrap();
    fs::write(&block, b"").unwrap();
    let mut child = f
        .command(env!("CARGO_BIN_EXE_desktop-rs"))
        .args([
            "-C",
            f.repo.to_str().unwrap(),
            "discard",
            "file",
            "--basis",
            plan["basis"].as_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !started.exists() && Instant::now() < deadline {
        assert!(
            child.try_wait().unwrap().is_none(),
            "discard exited before smudge started"
        );
        thread::sleep(Duration::from_millis(20));
    }
    if !started.exists() {
        let _ = child.kill();
        let _ = child.wait();
        panic!("smudge did not start");
    }
    f.run(
        "kill",
        &["-INT".into(), child.id().to_string().into()],
        None,
        false,
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("cancelled discard did not exit");
        }
        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(130));
    assert_eq!(fs::read(&file).unwrap(), b"edited\n");
    assert_eq!(fs::read(f.repo.join(".git/index")).unwrap(), index);
    assert!(!f.repo.join(".git/index.lock").exists());
    assert!(!f.repo.join(".git/HEAD.lock").exists());
    assert!(
        !fs::read_dir(f.repo.join(".git/desktop-undo"))
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".preparing-"))
    );
}
