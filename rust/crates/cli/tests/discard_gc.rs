//! Durable discard undo survives pruning and loss of split-index files.
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

struct Fixture {
    root: PathBuf,
    repo: PathBuf,
}

impl Fixture {
    fn new(root: PathBuf, name: &str) -> Self {
        let repo = root.join(name);
        let f = Self { root, repo };
        f.run_git(
            &["init", "-q", "-b", "main", f.repo.to_str().unwrap()],
            true,
        );
        f.git(&["config", "user.name", "Discard Edge Test"]);
        f.git(&["config", "user.email", "edges@example.invalid"]);
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
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.root.join("global"))
            .env("GIT_TERMINAL_PROMPT", "0");
        command
    }

    fn run_git(&self, args: &[&str], success: bool) -> Output {
        let output = self.command("git").args(args).output().unwrap();
        assert_eq!(
            output.status.success(),
            success,
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn git(&self, args: &[&str]) -> Output {
        let mut arguments = vec!["-C", self.repo.to_str().unwrap()];
        arguments.extend_from_slice(args);
        self.run_git(&arguments, true)
    }

    fn rpc(&self, request: Value) -> Value {
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
        assert!(
            output.status.success(),
            "{request}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn commit(&self) {
        self.git(&["add", "--all"]);
        self.git(&["commit", "-qm", "checkpoint"]);
    }

    fn discard(&self) -> String {
        let preview = self.rpc(json!({"method":"discard_preview", "path":"file", "scope":"all_changes", "selection":null}));
        let result = self.rpc(json!({"method":"discard", "path":"file", "scope":"all_changes", "selection":null, "basis":preview["basis"]}));
        result["undo_id"].as_str().unwrap().to_owned()
    }

    fn undo(&self, id: &str) {
        self.rpc(json!({"method":"undo_discard", "undo_id":id}));
    }
}

struct TestDirectory(PathBuf);
impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn read(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap()
}

#[test]
fn undo_restores_staged_blobs_and_split_index_after_pruning() {
    let root = TestDirectory(
        std::env::temp_dir().join(format!("desktop-discard-gc-{}", uuid::Uuid::new_v4())),
    );
    fs::create_dir_all(&root.0).unwrap();
    let f = Fixture::new(root.0.clone(), "objects");
    let file = f.repo.join("file");
    fs::write(&file, b"committed\n").unwrap();
    f.commit();
    fs::write(&file, b"staged only\n").unwrap();
    f.git(&["add", "--", "file"]);
    let oid = String::from_utf8(f.git(&["rev-parse", ":file"]).stdout).unwrap();
    fs::write(&file, b"working only\n").unwrap();
    let id = f.discard();
    f.git(&["prune", "--expire", "now"]);
    f.run_git(
        &["-C", f.repo.to_str().unwrap(), "cat-file", "-e", oid.trim()],
        false,
    );
    f.undo(&id);
    assert_eq!(f.git(&["show", ":file"]).stdout, b"staged only\n");
    assert_eq!(read(&file), b"working only\n");

    let f = Fixture::new(root.0.clone(), "shared");
    let file = f.repo.join("file");
    fs::write(&file, b"committed\n").unwrap();
    f.commit();
    f.git(&["config", "core.splitIndex", "true"]);
    f.git(&["update-index", "--split-index"]);
    fs::write(&file, b"staged flagged\n").unwrap();
    f.git(&["add", "--", "file"]);
    f.git(&["update-index", "--assume-unchanged", "--", "file"]);
    let before = read(&f.repo.join(".git/index"));
    let id = f.discard();
    let shared: Vec<_> = fs::read_dir(f.repo.join(".git"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("sharedindex.")
        })
        .collect();
    assert!(!shared.is_empty());
    f.git(&["prune", "--expire", "now"]);
    for path in shared {
        fs::remove_file(path).unwrap();
    }
    f.undo(&id);
    assert_eq!(read(&f.repo.join(".git/index")), before);
    assert_eq!(f.git(&["show", ":file"]).stdout, b"staged flagged\n");
    assert!(
        f.git(&["ls-files", "-v", "--", "file"])
            .stdout
            .starts_with(b"h ")
    );
    assert_eq!(read(&file), b"staged flagged\n");
}
