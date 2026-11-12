//! End-to-end captured diffs and transactional partial staging with real Git.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{
    ffi::OsStr,
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
        let root = std::env::temp_dir().join(format!("desktop-diff-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let f = Self {
            repo: root.join("repo"),
            root,
        };
        let output = f
            .command("git")
            .args(["init", "-q", "-b", "main"])
            .arg(&f.repo)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        f.git(&["config", "user.name", "Diff Test"]);
        f.git(&["config", "user.email", "diff@example.invalid"]);
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

    fn git_os(&self, args: &[&OsStr]) -> Output {
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
    fn git(&self, args: &[&str]) -> Output {
        self.git_os(&args.iter().map(OsStr::new).collect::<Vec<_>>())
    }
    fn cli(&self, args: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_desktop-rs"))
            .arg("-C")
            .arg(&self.repo)
            .args(args)
            .output()
            .unwrap()
    }
    fn rpc(&self, request: Value, success: bool) -> Value {
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
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            result.get("code").and_then(Value::as_i64).unwrap_or(0),
            0,
            "{result}"
        );
        result
    }
    fn detail(&self, path: Value, staged: bool) -> Value {
        self.rpc(
            json!({"method":"diff_detail", "path":path, "staged":staged}),
            true,
        )
    }
    fn select(&self, view: &Value, lines: Vec<Value>, success: bool) {
        self.rpc(json!({"method":"select_diff", "path":view["path"], "staged":view["staged"], "basis":view["basis"], "lines":lines}), success);
    }
    fn select_all(&self, view: &Value) {
        self.select(view, changes(view), true);
    }
    fn index(&self, path: &str) -> Vec<u8> {
        self.git(&["show", &format!(":{path}")]).stdout
    }
    fn commit(&self) {
        self.git(&["add", "--all"]);
        self.git(&["commit", "-qm", "checkpoint"]);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn changes(view: &Value) -> Vec<Value> {
    view["hunks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|hunk| hunk["lines"].as_array().unwrap())
        .filter(|line| line["kind"] == "added" || line["kind"] == "deleted")
        .map(|line| line["id"].clone())
        .collect()
}
fn hunk_changes(view: &Value, index: usize) -> Vec<Value> {
    view["hunks"][index]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|line| line["kind"] == "added" || line["kind"] == "deleted")
        .map(|line| line["id"].clone())
        .collect()
}

#[test]
fn captured_diffs_and_selective_index_transactions() {
    let mut f = Fixture::new();
    let file = f.repo.join("file.txt");
    let baseline = (0..60).map(|n| format!("line {n}\n")).collect::<String>();
    fs::write(&file, &baseline).unwrap();
    f.commit();
    let modified = baseline
        .replace("line 2\n", "changed two\n")
        .replace("line 50\n", "changed fifty\n");
    fs::write(&file, &modified).unwrap();
    fs::write(f.repo.join("unrelated"), "preserved").unwrap();
    f.git(&["add", "--", "unrelated"]);
    let unrelated = f.git(&["ls-files", "--stage", "--", "unrelated"]).stdout;
    let view = f.detail(json!("file.txt"), false);
    assert_eq!(view["kind"], "text");
    assert_eq!(view["hunks"].as_array().unwrap().len(), 2);
    assert_eq!(view["can_select"], true);
    let first = hunk_changes(&view, 0);
    f.select(&view, first.clone(), true);
    assert_eq!(
        f.index("file.txt"),
        baseline.replace("line 2\n", "changed two\n").as_bytes()
    );
    assert_eq!(fs::read(&file).unwrap(), modified.as_bytes());
    assert_eq!(
        f.git(&["ls-files", "--stage", "--", "unrelated"]).stdout,
        unrelated
    );
    f.select(&view, first, false);
    f.select_all(&f.detail(json!("file.txt"), false));
    assert_eq!(f.index("file.txt"), modified.as_bytes());
    let staged = f.detail(json!("file.txt"), true);
    assert_eq!(staged["hunks"].as_array().unwrap().len(), 2);
    f.select(&staged, hunk_changes(&staged, 0), true);
    assert_eq!(
        f.index("file.txt"),
        baseline.replace("line 50\n", "changed fifty\n").as_bytes()
    );
    f.select_all(&f.detail(json!("file.txt"), true));
    assert_eq!(f.index("file.txt"), baseline.as_bytes());
    assert_eq!(fs::read(&file).unwrap(), modified.as_bytes());

    let view = f.detail(json!("file.txt"), false);
    let old_index = fs::read(f.repo.join(".git/index")).unwrap();
    fs::write(&file, format!("{modified}external\n")).unwrap();
    f.select(&view, changes(&view), false);
    assert_eq!(fs::read(f.repo.join(".git/index")).unwrap(), old_index);
    let view = f.detail(json!("file.txt"), false);
    let context = view["hunks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|h| h["lines"].as_array().unwrap())
        .find(|line| line["kind"] == "context")
        .unwrap()["id"]
        .clone();
    f.select(&view, vec![context], false);
    f.select(&view, vec![json!(999999)], false);
    f.select(&view, vec![], false);
    let lock = f.repo.join(".git/index.lock");
    fs::write(&lock, b"other-owner").unwrap();
    f.select(&view, changes(&view), false);
    assert_eq!(fs::read(&lock).unwrap(), b"other-owner");
    fs::remove_file(lock).unwrap();
    assert!(!fs::read_dir(f.repo.join(".git")).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("desktop-diff-")
    }));

    let new = f.repo.join("new.txt");
    fs::write(&new, b"one\ntwo\nthree").unwrap();
    let view = f.detail(json!("new.txt"), false);
    let ids = changes(&view);
    f.select(&view, vec![ids[0].clone(), ids[2].clone()], true);
    assert_eq!(f.index("new.txt"), b"one\nthree");
    assert_eq!(fs::read(&new).unwrap(), b"one\ntwo\nthree");
    let view = f.detail(json!("new.txt"), true);
    f.select(&view, vec![changes(&view)[0].clone()], true);
    assert_eq!(f.index("new.txt"), b"three");
    f.select_all(&f.detail(json!("new.txt"), true));
    assert!(f.git(&["ls-files", "--", "new.txt"]).stdout.is_empty());

    #[cfg(unix)]
    {
        use std::{
            ffi::OsString,
            os::unix::ffi::{OsStrExt, OsStringExt},
        };
        for name in [
            b":(glob)*.txt".as_slice(),
            b"-flag",
            b"line\nbreak",
            b"raw-\xff",
        ] {
            let path = json!({"bytes":name});
            fs::write(f.repo.join(OsStr::from_bytes(name)), b"first\nlast").unwrap();
            f.select_all(&f.detail(path, false));
            let mut index = vec![b':'];
            index.extend_from_slice(name);
            assert_eq!(
                f.git_os(&[OsStr::new("show"), &OsString::from_vec(index)])
                    .stdout,
                b"first\nlast"
            );
        }
    }
    f.commit();
    let deleted_before = f.git(&["show", "HEAD:file.txt"]).stdout;
    fs::remove_file(&file).unwrap();
    let view = f.detail(json!("file.txt"), false);
    f.select(&view, vec![changes(&view)[0].clone()], true);
    let after_first_line = &deleted_before[deleted_before
        .iter()
        .position(|byte| *byte == b'\n')
        .unwrap()
        + 1..];
    assert_eq!(f.index("file.txt"), after_first_line);
    assert!(!file.exists());
    f.select_all(&f.detail(json!("file.txt"), false));
    assert!(f.git(&["ls-files", "--", "file.txt"]).stdout.is_empty());
    f.select_all(&f.detail(json!("file.txt"), true));
    assert_eq!(
        f.index("file.txt"),
        f.git(&["show", "HEAD:file.txt"]).stdout
    );
    assert!(!file.exists());
    f.git(&["restore", "--", "file.txt"]);

    f.git(&["config", "core.autocrlf", "true"]);
    fs::write(&file, b"alpha\nbeta\n").unwrap();
    f.commit();
    fs::write(&file, b"alpha\r\nBETA\r\n").unwrap();
    f.select_all(&f.detail(json!("file.txt"), false));
    assert_eq!(f.index("file.txt"), b"alpha\nBETA\n");
    assert_eq!(fs::read(&file).unwrap(), b"alpha\r\nBETA\r\n");
    f.git(&["config", "core.autocrlf", "false"]);
    fs::write(&file, b"alpha\nBETA\n").unwrap();
    f.commit();
    fs::write(f.repo.join("binary"), b"a\0b").unwrap();
    assert_eq!(f.detail(json!("binary"), false)["kind"], "binary");
    let image = STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Wl6SAAAAABJRU5ErkJggg==").unwrap();
    fs::write(f.repo.join("image.png"), &image).unwrap();
    let view = f.detail(json!("image.png"), false);
    assert_eq!(view["kind"], "image");
    assert_eq!(
        STANDARD
            .decode(view["after"]["image"]["data"].as_str().unwrap())
            .unwrap(),
        image
    );
    fs::write(
        f.repo.join(".gitattributes"),
        "*.dat -diff\n*.flt filter=custom\n",
    )
    .unwrap();
    fs::write(f.repo.join("file.dat"), "text marked binary").unwrap();
    assert_eq!(f.detail(json!("file.dat"), false)["kind"], "binary");
    fs::write(f.repo.join("file.flt"), "filtered text").unwrap();
    assert_eq!(f.detail(json!("file.flt"), false)["can_select"], false);
    fs::write(f.repo.join("large"), vec![b'x'; 1024 * 1024 + 1]).unwrap();
    assert_eq!(f.detail(json!("large"), false)["kind"], "too_large");
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let external = f.root.join("outside");
        fs::write(&external, "private").unwrap();
        symlink(&external, f.repo.join("link")).unwrap();
        assert_eq!(f.detail(json!("link"), false)["kind"], "symlink");
        symlink(&f.root, f.repo.join("linked-dir")).unwrap();
        f.rpc(
            json!({"method":"diff_detail", "path":"linked-dir/outside"}),
            false,
        );
    }
    f.rpc(json!({"method":"diff_detail", "path":".git/config"}), false);
    f.rpc(json!({"method":"diff_detail", "path":"../outside"}), false);

    f.git(&["mv", "file.txt", "renamed.txt"]);
    let renamed = f.repo.join("renamed.txt");
    fs::write(&renamed, b"alpha\nBETA\nextra\n").unwrap();
    f.select_all(&f.detail(json!("renamed.txt"), false));
    let view = f.detail(json!("renamed.txt"), true);
    assert_eq!(view["before"]["mode"], "100644");
    f.select_all(&view);
    assert_eq!(f.index("renamed.txt"), b"alpha\nBETA\n");
    assert!(!file.exists());
    f.git(&["update-index", "--split-index"]);
    fs::write(&renamed, b"alpha\nBETA\nchanged\n").unwrap();
    f.select_all(&f.detail(json!("renamed.txt"), false));
    assert_eq!(f.index("renamed.txt"), b"alpha\nBETA\nchanged\n");
    f.select_all(&f.detail(json!("renamed.txt"), true));
    assert_eq!(f.index("renamed.txt"), b"alpha\nBETA\n");

    let worktree = f.root.join("worktree");
    f.git(&[
        "worktree",
        "add",
        "-qb",
        "linked",
        worktree.to_str().unwrap(),
        "HEAD",
    ]);
    fs::write(worktree.join("file.txt"), b"linked content\n").unwrap();
    let old_repo = std::mem::replace(&mut f.repo, worktree);
    f.select_all(&f.detail(json!("file.txt"), false));
    assert_eq!(f.index("file.txt"), b"linked content\n");
    f.repo = old_repo;
    assert_eq!(f.index("renamed.txt"), b"alpha\nBETA\n");

    let sha = f.root.join("sha256");
    let output = f
        .command("git")
        .args(["init", "-q", "--object-format=sha256"])
        .arg(&sha)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    f.repo = sha;
    fs::write(f.repo.join("file.txt"), b"first\nlast").unwrap();
    f.select_all(&f.detail(json!("file.txt"), false));
    assert_eq!(f.index("file.txt"), b"first\nlast");
    f.select_all(&f.detail(json!("file.txt"), true));
    assert!(f.git(&["ls-files", "--", "file.txt"]).stdout.is_empty());
    fs::write(
        f.repo.join(".gitattributes"),
        "*.utf16 working-tree-encoding=UTF-16LE\n",
    )
    .unwrap();
    let encoded: Vec<u8> = "first\nsecond"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    fs::write(f.repo.join("file.utf16"), encoded).unwrap();
    f.select_all(&f.detail(json!("file.utf16"), false));
    assert_eq!(f.index("file.utf16"), b"first\nsecond");

    let structured = f.cli(&["diff", "--json", "file.txt"]);
    assert!(
        structured.status.success(),
        "{}",
        String::from_utf8_lossy(&structured.stderr)
    );
    let parsed: Value = serde_json::from_slice(&structured.stdout).unwrap();
    let lines = changes(&parsed)
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let selected = f.cli(&[
        "select-lines",
        "file.txt",
        "--basis",
        parsed["basis"].as_str().unwrap(),
        "--lines",
        &lines,
    ]);
    assert!(
        selected.status.success(),
        "{}",
        String::from_utf8_lossy(&selected.stderr)
    );
    assert_eq!(f.index("file.txt"), b"first\nlast");
}
