//! Creation ref transactions, hook delegation and case-only rename parity.
#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
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
            std::env::temp_dir().join(format!("desktop-branch-refs-{}", uuid::Uuid::new_v4()));
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
            .env_remove("GIT_LITERAL_PATHSPECS")
            .envs(&self.env);
        command
    }
    fn run<I, S>(&self, executable: &str, args: I, input: Option<&[u8]>, fail: bool) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = self.command(executable);
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
    fn init(&self, name: &str, options: &[&str], committed: bool) -> PathBuf {
        let repo = self.root.join(name);
        let mut args: Vec<OsString> = ["init", "-q", "-b", "main"]
            .into_iter()
            .map(Into::into)
            .collect();
        args.extend(options.iter().map(OsString::from));
        args.push(repo.as_os_str().into());
        self.run("git", args, None, false);
        self.git(&repo, &["config", "user.name", "Creation Test"]);
        self.git(&repo, &["config", "user.email", "create@example.invalid"]);
        self.git(&repo, &["config", "commit.gpgsign", "false"]);
        if committed {
            fs::write(
                repo.join("file"),
                (0..30).map(|i| format!("line {i}\n")).collect::<String>(),
            )
            .unwrap();
            self.commit(&repo);
        }
        repo
    }
    fn commit(&self, repo: &Path) {
        self.git(repo, &["add", "--all"]);
        self.git(repo, &["commit", "-qm", "checkpoint"]);
    }
    fn rpc_raw(&self, repo: &Path, method: &str, mut body: Value, fail: bool) -> Output {
        body["method"] = json!(method);
        self.run(
            env!("CARGO_BIN_EXE_desktop-rs"),
            [
                OsString::from("-C"),
                repo.as_os_str().into(),
                "repo".into(),
                "-".into(),
            ],
            Some(body.to_string().as_bytes()),
            fail,
        )
    }
    fn rpc(&self, repo: &Path, method: &str, body: Value) -> Value {
        serde_json::from_slice(&self.rpc_raw(repo, method, body, false).stdout).unwrap()
    }
    fn review(&self, repo: &Path, name: &str, params: &Value) -> Value {
        let mut body = json!({"name":name});
        for key in ["start_point", "checkout", "no_track"] {
            if let Some(value) = params.get(key) {
                body[key] = value.clone();
            }
        }
        self.rpc(repo, "create_branch_preview", body)
    }
    fn create_raw(&self, repo: &Path, name: &str, mut params: Value, fail: bool) -> Output {
        let plan = self.review(repo, name, &params);
        params["name"] = json!(name);
        params["basis"] = plan["basis"].clone();
        self.rpc_raw(repo, "create_branch", params, fail)
    }
    fn create(&self, repo: &Path, name: &str, params: Value) -> Value {
        serde_json::from_slice(&self.create_raw(repo, name, params, false).stdout).unwrap()
    }
    fn wrapper(&mut self, repo: &Path, label: &str, script: &str) {
        let directory = self.root.join(label);
        fs::create_dir(&directory).unwrap();
        let tool = directory.join("git");
        fs::write(&tool, script).unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o700)).unwrap();
        let real = String::from_utf8(self.run("sh", ["-c", "command -v git"], None, false).stdout)
            .unwrap()
            .trim()
            .to_owned();
        let path = std::env::join_paths(std::iter::once(directory).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )))
        .unwrap();
        self.env.insert("PATH".into(), path);
        self.env.insert("CREATE_REAL_GIT".into(), real.into());
        self.env
            .insert("CREATE_REPO".into(), repo.as_os_str().into());
    }
    fn clear_wrapper(&mut self) {
        for key in [
            "PATH",
            "CREATE_REAL_GIT",
            "CREATE_REPO",
            "CREATE_MODE",
            "CREATE_MARKER",
            "CREATE_TIP",
            "CREATE_NAME",
        ] {
            self.env.remove(OsStr::new(key));
        }
    }
}
impl Drop for Suite {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
const LEAVE: &str = "StashOnCurrentBranch";

const CREATE_WRAPPER: &str = "#!/bin/sh\nbranch=0; target=0; hook=0; probe=0\nfor arg in \"$@\"; do\n [ \"$arg\" = branch ] && branch=1\n [ \"$arg\" = \"$CREATE_NAME\" ] && target=1\n [ \"$arg\" = hook ] && hook=1\n case \"$arg\" in desktop-branch-probe-*) probe=1;; esac\ndone\nif [ \"$CREATE_MODE\" = no-hook ] && [ \"$hook$probe\" = 11 ]; then exit 0; fi\nif [ \"$branch$target\" = 11 ]; then\n case \"$CREATE_MODE\" in\n  symbolic) \"$CREATE_REAL_GIT\" -C \"$CREATE_REPO\" symbolic-ref refs/heads/new refs/heads/unborn-other || exit 95;;\n  start|disabled) \"$CREATE_REAL_GIT\" -C \"$CREATE_REPO\" update-ref refs/heads/start \"$CREATE_TIP\" || exit 95;;\n  direct) \"$CREATE_REAL_GIT\" -C \"$CREATE_REPO\" update-ref refs/heads/new \"$CREATE_TIP\" || exit 95;;\n  no-hook) printf invoked > \"$CREATE_MARKER\";;\n esac\nfi\nexec \"$CREATE_REAL_GIT\" \"$@\"\n";

fn head(s: &Suite, repo: &Path, name: &str) -> Option<String> {
    let output = s
        .command("git")
        .args([
            "-C",
            repo.to_str().unwrap(),
            "show-ref",
            "--verify",
            "--hash",
            &format!("refs/heads/{name}"),
        ])
        .output()
        .unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).unwrap().trim().to_owned())
}

#[test]
fn symbolic_destinations_and_creation_writer_races() {
    let mut s = Suite::new();
    for backend in ["files", "sha256", "reftable"] {
        let options: Vec<&str> = match backend {
            "sha256" => vec!["--object-format=sha256"],
            "reftable" => vec!["--ref-format=reftable"],
            _ => vec![],
        };
        let repo = s.init(&format!("symbolic-{backend}"), &options, true);
        s.git(
            &repo,
            &["symbolic-ref", "refs/heads/new", "refs/heads/unborn-other"],
        );
        fs::write(repo.join("file"), "keep index").unwrap();
        s.git(&repo, &["add", "--", "file"]);
        fs::write(repo.join("file"), "keep working").unwrap();
        let index = fs::read(repo.join(".git/index")).unwrap();
        for checkout in [false, true] {
            s.rpc_raw(
                &repo,
                "create_branch_preview",
                json!({"name":"new","checkout":checkout}),
                true,
            );
        }
        s.rpc_raw(
            &repo,
            "rename_branch_preview",
            json!({"name":"main","new_name":"new"}),
            true,
        );
        assert!(head(&s, &repo, "unborn-other").is_none());
        assert_eq!(
            s.git(&repo, &["symbolic-ref", "refs/heads/new"]),
            "refs/heads/unborn-other"
        );
        assert_eq!(fs::read(repo.join(".git/index")).unwrap(), index);
        assert_eq!(fs::read(repo.join("file")).unwrap(), b"keep working");
        assert!(!repo.join(".git/desktop-checkout").exists());
        assert!(!repo.join(".git/desktop-branch-renames").exists());
        for kind in ["symbolic", "start", "direct"] {
            let repo = s.init(&format!("race-{backend}-{kind}"), &options, true);
            s.git(&repo, &["branch", "start"]);
            let tip = s.git(&repo, &["commit-tree", "HEAD^{tree}", "-m", "writer"]);
            fs::write(repo.join("file"), "working").unwrap();
            fs::write(repo.join("loose"), "loose").unwrap();
            let index = fs::read(repo.join(".git/index")).unwrap();
            let plan = s.review(
                &repo,
                "new",
                &json!({"start_point":"refs/heads/start","checkout":true}),
            );
            s.wrapper(&repo, &format!("tool-{backend}-{kind}"), CREATE_WRAPPER);
            s.env.insert("CREATE_MODE".into(), kind.into());
            s.env.insert("CREATE_NAME".into(), "new".into());
            s.env.insert("CREATE_TIP".into(), tip.clone().into());
            s.rpc_raw(&repo, "create_branch", json!({"name":"new","start_point":"refs/heads/start","checkout":true,"strategy":LEAVE,"basis":plan["basis"]}), true);
            s.clear_wrapper();
            s.env.remove(OsStr::new("CREATE_TIP"));
            assert!(head(&s, &repo, "unborn-other").is_none());
            assert_eq!(fs::read(repo.join(".git/index")).unwrap(), index);
            assert_eq!(fs::read(repo.join("file")).unwrap(), b"working");
            assert_eq!(fs::read(repo.join("loose")).unwrap(), b"loose");
            assert_eq!(s.rpc(&repo, "stash_list", json!({}))["entries"], json!([]));
            assert_eq!(head(&s, &repo, "new"), (kind == "direct").then_some(tip));
            assert_eq!(
                s.rpc(&repo, "checkout_history", json!({}))[0]["receipt"]["phase"],
                "creating_branch"
            );
        }
    }
    let repo = s.init("remote-preview", &[], true);
    let tip = head(&s, &repo, "main").unwrap();
    s.git(&repo, &["update-ref", "refs/remotes/origin/topic", &tip]);
    s.git(
        &repo,
        &["symbolic-ref", "refs/heads/topic", "refs/heads/other"],
    );
    s.rpc_raw(
        &repo,
        "checkout_preview",
        json!({"branch":"refs/remotes/origin/topic"}),
        true,
    );
    assert!(head(&s, &repo, "other").is_none());
    let repo = s.init("unborn-symbolic", &[], false);
    s.git(
        &repo,
        &["symbolic-ref", "refs/heads/new", "refs/heads/other"],
    );
    s.rpc_raw(
        &repo,
        "create_branch_preview",
        json!({"name":"new","checkout":true}),
        true,
    );
    assert_eq!(s.git(&repo, &["symbolic-ref", "HEAD"]), "refs/heads/main");
    let repo = s.init("hook-not-run", &[], true);
    let marker = s.root.join("must-not-create");
    s.wrapper(&repo, "no-hook-tool", CREATE_WRAPPER);
    s.env.insert("CREATE_MODE".into(), "no-hook".into());
    s.env.insert("CREATE_NAME".into(), "new".into());
    s.env
        .insert("CREATE_MARKER".into(), marker.as_os_str().into());
    s.create_raw(&repo, "new", json!({}), true);
    s.clear_wrapper();
    assert!(!marker.exists());
    assert!(head(&s, &repo, "new").is_none());
}

#[test]
fn hook_logger() {
    use std::io::Read;
    let Some(path) = std::env::var_os("REFS_HOOK_LOG") else {
        return;
    };
    let phase = std::env::var("REFS_HOOK_PHASE").unwrap();
    let named = std::env::var("REFS_HOOK_NAMED").unwrap_or_default();
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).unwrap();
    let mut args = vec![phase];
    if !named.is_empty() {
        args.push(named);
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(file, "{}", json!([args, input])).unwrap();
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn events(path: &Path, name: &str) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|line| {
            let mut value: Value = serde_json::from_str(line).unwrap();
            value[1] = json!(
                value[1]
                    .as_str()
                    .unwrap()
                    .replace(&format!("refs/heads/{name}"), "refs/heads/BRANCH")
            );
            value
        })
        .collect()
}

#[test]
fn transaction_hooks_are_delegated_and_cannot_redirect_creation() {
    let mut s = Suite::new();
    let repo = s.init("hook-race", &[], true);
    s.git(&repo, &["branch", "start"]);
    let tip = s.git(&repo, &["commit-tree", "HEAD^{tree}", "-m", "writer"]);
    let hook = repo.join(".git/hooks/reference-transaction");
    fs::write(&hook, "#!/bin/sh\ndata=$(cat)\ncase \"$1:$data\" in preparing:*\" refs/heads/new\") git update-ref refs/heads/start \"$REFS_HOOK_TIP\" || exit 95;; esac\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
    s.env.insert("REFS_HOOK_TIP".into(), tip.clone().into());
    s.create_raw(
        &repo,
        "new",
        json!({"start_point":"refs/heads/start"}),
        true,
    );
    s.env.remove(OsStr::new("REFS_HOOK_TIP"));
    assert!(head(&s, &repo, "new").is_none());
    assert_eq!(head(&s, &repo, "start"), Some(tip));
    let repo = s.init("prepared-destination", &[], true);
    let hook = repo.join(".git/hooks/reference-transaction");
    fs::write(&hook, "#!/bin/sh\ndata=$(cat)\ncase \"$data\" in *ref:*) exit 0;; esac\ncase \"$1:$data\" in preparing:*\" refs/heads/new\") git symbolic-ref refs/heads/new refs/heads/unborn-other || exit 95;; esac\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
    s.create_raw(&repo, "new", json!({}), true);
    assert!(head(&s, &repo, "unborn-other").is_none());
    assert_eq!(
        s.git(&repo, &["symbolic-ref", "refs/heads/new"]),
        "refs/heads/unborn-other"
    );
    let repo = s.init("quoted-refs", &[], true);
    let source = "start/'$(touch${IFS}PWNED)`";
    let name = "new/'$(touch${IFS}PWNED)`";
    s.git(&repo, &["branch", "--", source]);
    s.env.insert("GIT_CONFIG_PARAMETERS".into(), "'desktop.private-token'='private-secret-value $(touch${IFS}PWNED)' 'hook.reference-transaction.enabled'='false'".into());
    let result = s.create(
        &repo,
        name,
        json!({"start_point":format!("refs/heads/{source}")}),
    );
    assert_eq!(head(&s, &repo, name), head(&s, &repo, "main"));
    assert!(!repo.join("PWNED").exists());
    assert!(
        !s.rpc(&repo, "checkout_history", json!({}))
            .to_string()
            .contains("private-secret-value")
    );
    s.rpc(
        &repo,
        "acknowledge_checkout",
        json!({"receipt_id":result["receipt_id"]}),
    );
    s.env.remove(OsStr::new("GIT_CONFIG_PARAMETERS"));
    for disabled in ["none", "friendly", "event"] {
        let repo = s.init(&format!("callbacks-{disabled}"), &[], true);
        let log = s.root.join(format!("callback-log-{disabled}"));
        let hooks = s.root.join(format!("hooks 'quote\n{disabled}"));
        fs::create_dir(&hooks).unwrap();
        let hook = hooks.join("reference-transaction");
        let exe = std::env::current_exe().unwrap();
        let script = format!(
            "#!/bin/sh\nexport REFS_HOOK_PHASE=\"$1\"\nexport REFS_HOOK_NAMED=\"${{2-}}\"\nexec {} --exact hook_logger --nocapture >/dev/null\n",
            quote(exe.to_str().unwrap())
        );
        fs::write(&hook, script).unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
        s.env.insert("REFS_HOOK_LOG".into(), log.as_os_str().into());
        s.git(
            &repo,
            &["config", "core.hooksPath", hooks.to_str().unwrap()],
        );
        s.git(
            &repo,
            &["config", "hook.named.event", "reference-transaction"],
        );
        s.git(
            &repo,
            &[
                "config",
                "hook.named.command",
                &format!("{} named", quote(hook.to_str().unwrap())),
            ],
        );
        if disabled == "friendly" {
            s.git(&repo, &["config", "hook.named.enabled", "false"]);
        }
        if disabled == "event" {
            s.git(
                &repo,
                &["config", "hook.reference-transaction.enabled", "false"],
            );
        }
        s.git(&repo, &["branch", "native"]);
        let expected = events(&log, "native");
        s.git(&repo, &["branch", "-D", "native"]);
        fs::write(&log, b"").unwrap();
        let result = s.create(&repo, "new", json!({}));
        assert_eq!(events(&log, "new"), expected);
        let id = result["receipt_id"].as_str().unwrap();
        let guard = repo
            .join(".git/desktop-checkout")
            .join(format!("{id}.create-hooks"));
        assert!(guard.is_dir());
        assert_eq!(fs::metadata(&guard).unwrap().permissions().mode() & 0o77, 0);
        assert_eq!(
            fs::metadata(guard.join("reference-transaction"))
                .unwrap()
                .permissions()
                .mode()
                & 0o77,
            0
        );
        s.rpc(&repo, "acknowledge_checkout", json!({"receipt_id":id}));
        assert!(!guard.exists());
        if disabled == "event" {
            s.git(&repo, &["branch", "start"]);
            let tip = s.git(&repo, &["commit-tree", "HEAD^{tree}", "-m", "writer"]);
            let plan = s.review(&repo, "raced", &json!({"start_point":"refs/heads/start"}));
            s.wrapper(&repo, "disabled-shim", CREATE_WRAPPER);
            s.env.insert("CREATE_MODE".into(), "disabled".into());
            s.env.insert("CREATE_NAME".into(), "raced".into());
            s.env.insert("CREATE_TIP".into(), tip.into());
            s.rpc_raw(
                &repo,
                "create_branch",
                json!({"name":"raced","start_point":"refs/heads/start","basis":plan["basis"]}),
                true,
            );
            s.clear_wrapper();
            assert!(head(&s, &repo, "raced").is_none());
            assert_eq!(
                s.git(
                    &repo,
                    &["config", "--bool", "hook.reference-transaction.enabled"]
                ),
                "false"
            );
        }
        s.env.remove(OsStr::new("REFS_HOOK_LOG"));
    }
    let repo = s.init("nested-callback", &[], true);
    let other = s.init("nested-other", &[], true);
    let marker = s.root.join("nested-marker");
    let hook = other.join(".git/hooks/reference-transaction");
    fs::write(&hook, "#!/bin/sh\nprintf done > \"$REFS_NESTED_MARKER\"\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
    let hook = repo.join(".git/hooks/reference-transaction");
    fs::write(&hook, "#!/bin/sh\ncat >/dev/null\nif [ \"$1\" = preparing ]; then\n unset GIT_DIR GIT_WORK_TREE GIT_COMMON_DIR GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_PREFIX\n git -C \"$REFS_NESTED_REPO\" branch nested || exit 95\nfi\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
    s.env
        .insert("REFS_NESTED_MARKER".into(), marker.as_os_str().into());
    s.env
        .insert("REFS_NESTED_REPO".into(), other.as_os_str().into());
    s.create(&repo, "new", json!({}));
    assert!(marker.exists());
    assert!(head(&s, &other, "nested").is_some());
}

const CASE_WRAPPER: &str = "#!/bin/sh\nshow=0; hash=0; branch=0; move=0; force=0; last=\nfor arg in \"$@\"; do\n last=$arg\n case \"$arg\" in show-ref) show=1;; --hash) hash=1;; branch) branch=1;; -m) move=1;; -M) force=1;; esac\ndone\nif [ \"$CASE_MODE\" = alias ] && [ \"$show$hash\" = 11 ] && [ \"$last\" = \"refs/heads/$CASE_DEST\" ]; then\n \"$CREATE_REAL_GIT\" -C \"$CREATE_REPO\" show-ref --verify --hash \"refs/heads/$CASE_SOURCE\" && exit 0\nfi\nif [ \"$branch\" = 1 ] && { [ \"$move\" = 1 ] || [ \"$force\" = 1 ]; }; then\n if [ \"$force\" = 1 ]; then printf '%s\\n' -M >> \"$CASE_LOG\"; else printf '%s\\n' -m >> \"$CASE_LOG\"; fi\n if [ \"$move\" = 1 ]; then\n  case \"$CASE_MODE\" in\n   source-race) \"$CREATE_REAL_GIT\" -C \"$CREATE_REPO\" update-ref \"refs/heads/$CASE_SOURCE\" \"$CASE_TIP\" || exit 95;;\n   destination-race) \"$CREATE_REAL_GIT\" -C \"$CREATE_REPO\" branch -- \"$CASE_DEST\" \"$CASE_TIP\" || exit 95;;\n   config-race) \"$CREATE_REAL_GIT\" -C \"$CREATE_REPO\" config \"branch.$CASE_SOURCE.description\" changed || exit 95;;\n   hook-detail) printf '%s\\n' 'fatal: ref updates aborted by hook' >&2;;\n  esac\n  printf \"fatal: a branch named '%s' already exists\\n\" \"$CASE_DEST\" >&2\n  [ \"$CASE_MODE\" = hook-refusal ] && exit 1\n  exit 128\n fi\nfi\nexec \"$CREATE_REAL_GIT\" \"$@\"\n";

#[test]
fn case_only_rename_retries_only_verified_collisions() {
    let mut s = Suite::new();
    for kind in [
        "normal",
        "fallback",
        "alias",
        "unicode",
        "exact",
        "other-case",
        "source-race",
        "destination-race",
        "config-race",
        "hook-refusal",
        "hook-detail",
    ] {
        let repo = s.init(&format!("case-{kind}"), &[], true);
        let (source, dest) = if kind == "unicode" {
            ("fÖÖ's", "Föö'S")
        } else {
            ("main", "MAIN")
        };
        if source != "main" {
            s.git(&repo, &["branch", "-m", source]);
        }
        s.git(
            &repo,
            &["config", &format!("branch.{source}.description"), "saved"],
        );
        fs::write(repo.join("file"), "saved").unwrap();
        let original = s.rpc(&repo, "stash_save", json!({"desktop":true}))["oid"].clone();
        fs::write(repo.join("file"), "untouched").unwrap();
        let index = fs::read(repo.join(".git/index")).unwrap();
        let tip = head(&s, &repo, source);
        if kind == "exact" {
            s.git(&repo, &["branch", dest]);
            s.rpc_raw(
                &repo,
                "rename_branch_preview",
                json!({"name":source,"new_name":dest}),
                true,
            );
            assert_eq!(head(&s, &repo, source), tip);
            assert_eq!(head(&s, &repo, dest), tip);
            continue;
        }
        if kind == "other-case" {
            s.git(&repo, &["branch", "mAiN"]);
        }
        let plan = s.rpc(
            &repo,
            "rename_branch_preview",
            json!({"name":source,"new_name":dest}),
        );
        assert_eq!(plan["case_only"], true);
        assert_eq!(plan["case_retry"], kind != "other-case");
        let log = s.root.join(format!("case-log-{kind}"));
        let new = s.git(&repo, &["commit-tree", "HEAD^{tree}", "-m", "writer"]);
        if kind == "normal" {
            s.rpc(
                &repo,
                "rename_branch",
                json!({"name":source,"new_name":dest,"basis":plan["basis"]}),
            );
        } else {
            s.wrapper(&repo, &format!("case-tool-{kind}"), CASE_WRAPPER);
            for (key, value) in [
                ("CASE_MODE", OsString::from(kind)),
                ("CASE_SOURCE", source.into()),
                ("CASE_DEST", dest.into()),
                ("CASE_TIP", new.clone().into()),
                ("CASE_LOG", log.as_os_str().into()),
            ] {
                s.env.insert(key.into(), value);
            }
            let success = ["fallback", "alias", "unicode"].contains(&kind);
            s.rpc_raw(
                &repo,
                "rename_branch",
                json!({"name":source,"new_name":dest,"basis":plan["basis"]}),
                !success,
            );
            s.clear_wrapper();
            for key in [
                "CASE_MODE",
                "CASE_SOURCE",
                "CASE_DEST",
                "CASE_TIP",
                "CASE_LOG",
            ] {
                s.env.remove(OsStr::new(key));
            }
            let actual = fs::read_to_string(&log).unwrap();
            assert_eq!(
                actual.lines().collect::<Vec<_>>(),
                if success {
                    vec!["-m", "-M"]
                } else {
                    vec!["-m"]
                }
            );
            if !success {
                assert_eq!(
                    head(&s, &repo, source),
                    if kind == "source-race" {
                        Some(new.clone())
                    } else {
                        tip.clone()
                    }
                );
                assert_eq!(
                    head(&s, &repo, dest),
                    (kind == "destination-race").then_some(new)
                );
                assert!(
                    s.rpc(&repo, "stash_list", json!({}))["entries"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|entry| entry["oid"] == original)
                );
                continue;
            }
        }
        assert!(head(&s, &repo, source).is_none());
        assert_eq!(head(&s, &repo, dest), tip);
        assert_eq!(
            s.git(&repo, &["symbolic-ref", "HEAD"]),
            format!("refs/heads/{dest}")
        );
        assert_eq!(
            s.git(&repo, &["config", &format!("branch.{dest}.description")]),
            "saved"
        );
        assert_eq!(fs::read(repo.join("file")).unwrap(), b"untouched");
        assert_eq!(fs::read(repo.join(".git/index")).unwrap(), index);
        let saved = s.rpc(&repo, "stash_list", json!({}));
        let saved = saved["entries"].as_array().unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0]["branch"], dest);
        assert_eq!(
            s.git_result(
                &repo,
                &[
                    "show",
                    &format!("{}:file", saved[0]["oid"].as_str().unwrap())
                ],
                false
            )
            .stdout,
            b"saved"
        );
    }
}
