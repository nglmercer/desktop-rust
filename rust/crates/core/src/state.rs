//! Branch tracking and in-progress Git operations, including linked worktrees.
use crate::{Repository, args, os_from_bytes};
use anyhow::{Context, Result, bail};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{fs, io::Read, path::PathBuf};

fn read_operation_file(dir: &Dir, name: &str, limit: u64) -> Result<Option<String>> {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = match dir.open_with(name, &options) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file() && metadata.len() <= limit,
        "invalid Git operation state file"
    );
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= limit,
        "Git operation state file is too large"
    );
    Ok(Some(
        String::from_utf8(bytes).context("invalid Git operation state encoding")?,
    ))
}

#[derive(Debug, Serialize)]
pub struct OperationState {
    pub kind: String,
    pub step: Option<u64>,
    pub total: Option<u64>,
}
impl Repository {
    /// Modification time of the worktree's nonempty FETCH_HEAD, in Unix milliseconds.
    pub(crate) async fn renderer_last_fetch(&self) -> Result<Value> {
        let path = self.git_directory().await?.join("FETCH_HEAD");
        let metadata = match tokio::fs::metadata(path).await {
            Ok(metadata) if metadata.len() == 0 => return Ok(json!({"kind":"empty"})),
            Ok(metadata) => metadata,
            Err(_) => return Ok(json!({"kind":"unavailable"})),
        };
        let modified = match metadata.modified() {
            Ok(modified) => modified,
            Err(_) => return Ok(json!({"kind":"unavailable"})),
        };
        let millis = match modified.duration_since(std::time::UNIX_EPOCH) {
            Ok(duration) => i128::try_from(duration.as_millis())?,
            Err(error) => -i128::try_from(error.duration().as_millis())?,
        };
        if !(-8_640_000_000_000_000..=8_640_000_000_000_000).contains(&millis) {
            return Ok(json!({"kind":"unavailable"}));
        }
        Ok(json!({"kind":"fetched", "time":i64::try_from(millis)?}))
    }

    pub(crate) async fn status_context(&self) -> Result<Value> {
        let directory = self.git_directory().await?;
        let state = self.renderer_rebase_state().await?;
        let progress = &state["progress"];
        let rebase = match (
            progress["original"].as_str(),
            progress["branch"].as_str(),
            progress["onto"].as_str(),
        ) {
            (Some(original), Some(branch), Some(base)) => {
                let branch = branch.strip_prefix("refs/heads/").map_or(branch, str::trim);
                json!({"originalBranchTip":original.trim(), "targetBranch":branch, "baseBranchTip":base.trim()})
            }
            _ => Value::Null,
        };
        Ok(json!({
            "mergeHeadFound":directory.join("MERGE_HEAD").exists(),
            "squashMsgFound":directory.join("SQUASH_MSG").exists(),
            "isCherryPickingHeadFound":directory.join("CHERRY_PICK_HEAD").exists(),
            "rebaseInternalState":rebase,
        }))
    }
    /// Read only the sequencer files needed by the renderer, without following
    /// links or exposing arbitrary Git-directory files to a browser client.
    pub(crate) async fn renderer_cherry_pick_state(&self) -> Result<Value> {
        let directory = Dir::open_ambient_dir(self.git_directory().await?, ambient_authority())?;
        let Some(head) = read_operation_file(&directory, "CHERRY_PICK_HEAD", 128)? else {
            return Ok(Value::Null);
        };
        let head = head.trim();
        anyhow::ensure!(
            (head.len() == 40 || head.len() == 64) && head.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid cherry-pick head"
        );
        fn read_sequence(directory: &Dir) -> Result<Option<Value>> {
            Ok(match directory.open_dir_nofollow("sequencer") {
                Ok(dir) => {
                    let head = read_operation_file(&dir, "head", 128)?;
                    let safety = read_operation_file(&dir, "abort-safety", 128)?;
                    let todo = read_operation_file(&dir, "todo", 1024 * 1024)?;
                    match (head, safety, todo) {
                        (Some(head), Some(safety), Some(todo)) => {
                            Some(json!({"head":head,"abort_safety":safety,"todo":todo}))
                        }
                        _ => None,
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            })
        }
        let sequence = read_sequence(&directory)?;
        if read_sequence(&directory)? != sequence {
            return Ok(Value::Null);
        }
        if read_operation_file(&directory, "CHERRY_PICK_HEAD", 128)?
            .as_deref()
            .map(str::trim)
            != Some(head)
        {
            return Ok(Value::Null);
        }
        Ok(json!({"head":head,"sequence":sequence}))
    }

    /// Read rebase progress and identity from fixed, bounded Git state files.
    pub(crate) async fn renderer_rebase_state(&self) -> Result<Value> {
        fn read_progress(root: &Dir) -> Result<Option<Value>> {
            let dir = match root.open_dir_nofollow("rebase-merge") {
                Ok(dir) => dir,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            Ok(Some(json!({
                "original": read_operation_file(&dir, "orig-head", 8192)?,
                "branch": read_operation_file(&dir, "head-name", 8192)?,
                "onto": read_operation_file(&dir, "onto", 8192)?,
                "current": read_operation_file(&dir, "msgnum", 8192)?,
                "total": read_operation_file(&dir, "end", 8192)?,
            })))
        }
        let root = Dir::open_ambient_dir(self.git_directory().await?, ambient_authority())?;
        let Some(head) = read_operation_file(&root, "REBASE_HEAD", 128)? else {
            return Ok(Value::Null);
        };
        let head = head.trim();
        anyhow::ensure!(
            (head.len() == 40 || head.len() == 64)
                && head.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "invalid rebase head"
        );
        let merge_progress = read_progress(&root)?;
        // Git may leave REBASE_HEAD behind after finishing. Require a live
        // rebase directory rather than treating that stale pseudoref as an
        // active operation. `rebase-apply/applying` denotes git am instead.
        let apply_state = || -> Result<(bool, bool, Option<Value>)> {
            let apply = match root.open_dir_nofollow("rebase-apply") {
                Ok(dir) => dir,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok((false, false, None));
                }
                Err(error) => return Err(error.into()),
            };
            let is_rebase = read_operation_file(&apply, "applying", 128)?.is_none();
            let progress = if is_rebase && read_operation_file(&apply, "rebasing", 128)?.is_some() {
                Some(json!({
                    "original": read_operation_file(&apply, "orig-head", 8192)?,
                    "branch": read_operation_file(&apply, "head-name", 8192)?,
                    "onto": read_operation_file(&apply, "onto", 8192)?,
                    "current": read_operation_file(&apply, "next", 8192)?,
                    "total": read_operation_file(&apply, "last", 8192)?,
                }))
            } else {
                None
            };
            Ok((true, is_rebase, progress))
        };
        let apply = apply_state()?;
        anyhow::ensure!(
            merge_progress.is_none() || !apply.0,
            "Ambiguous rebase state"
        );
        if merge_progress.is_none() && !apply.1 {
            return Ok(Value::Null);
        }
        if read_progress(&root)? != merge_progress
            || apply_state()? != apply
            || read_operation_file(&root, "REBASE_HEAD", 128)?
                .as_deref()
                .map(str::trim)
                != Some(head)
        {
            return Ok(Value::Null);
        }
        let backend = if merge_progress.is_some() {
            "merge"
        } else {
            "apply"
        };
        Ok(json!({"head":head,"backend":backend,"progress":merge_progress.or(apply.2)}))
    }

    pub(crate) async fn ahead_behind(&self, range: &str) -> Result<Value> {
        let output = self
            .run(&args(&[
                "rev-list",
                "--left-right",
                "--count",
                "--end-of-options",
                range,
                "--",
            ]))
            .await?;
        if output.code != 0 {
            if output.bad_revision() {
                return Ok(Value::Null);
            }
            bail!("{}", output.text().stderr);
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let Some((ahead, behind)) = text.split_once('\t') else {
            return Ok(Value::Null);
        };
        match (ahead.trim().parse::<u64>(), behind.trim().parse::<u64>()) {
            (Ok(ahead), Ok(behind)) => Ok(json!({ "ahead": ahead, "behind": behind })),
            _ => Ok(Value::Null),
        }
    }

    pub(crate) async fn branches_merged(&self, branch: &str) -> Result<Value> {
        let output = self
            .run(&args(&[
                "branch",
                "--format=%(objectname)%00%(refname)%00",
                &format!("--merged={branch}"),
                "--",
            ]))
            .await?;
        if output.code != 0 {
            bail!("{}", output.text().stderr);
        }
        let mut fields = output.stdout.split(|byte| *byte == 0);
        let mut branches = Vec::new();
        while let Some(oid) = fields.next() {
            let oid = oid.strip_prefix(b"\n").unwrap_or(oid);
            if oid.is_empty() {
                break;
            }
            let reference = fields.next().context("missing merged branch ref")?;
            branches.push(json!({"oid":String::from_utf8_lossy(oid), "ref":String::from_utf8_lossy(reference)}));
        }
        Ok(Value::Array(branches))
    }

    pub(crate) async fn branches_pointing_at(&self, commitish: &str) -> Result<Value> {
        let output = self
            .run(&args(&[
                "branch",
                &format!("--points-at={commitish}"),
                "--format=%(refname:short)",
                "--",
            ]))
            .await?;
        match output.code {
            0 => Ok(json!(
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .collect::<Vec<_>>()
            )),
            1 | 129 => Ok(Value::Null),
            _ => bail!("{}", output.text().stderr),
        }
    }

    pub(crate) async fn branch_enumeration(&self, prefixes: &[String]) -> Result<Value> {
        let defaults = ["refs/heads".to_owned(), "refs/remotes".to_owned()];
        let prefixes = if prefixes.is_empty() {
            &defaults[..]
        } else {
            prefixes
        };
        let mut arguments = args(&[
            "for-each-ref",
            "--format=%(refname)%00%(refname:short)%00%(upstream:short)%00%(objectname)%00%(symref)%00%(upstream)%00%(HEAD)%00",
            "--",
        ]);
        arguments.extend(prefixes.iter().map(std::ffi::OsString::from));
        let output = self.run(&arguments).await?;
        if output.code != 0 {
            bail!("{}", output.text().stderr);
        }
        let mut fields = output.stdout.split(|byte| *byte == 0);
        let mut branches = Vec::new();
        while let Some(reference) = fields.next() {
            let reference = reference.strip_prefix(b"\n").unwrap_or(reference);
            if reference.is_empty() {
                break;
            }
            let name = fields.next().context("missing branch short name")?;
            let upstream = fields.next().context("missing branch upstream")?;
            let oid = fields.next().context("missing branch oid")?;
            let symbolic = fields.next().context("missing symbolic branch target")?;
            let upstream_ref = fields.next().context("missing canonical upstream")?;
            let current = fields.next().context("missing current branch flag")? == b"*";
            if symbolic.is_empty() {
                branches.push(json!({
                    "ref": String::from_utf8_lossy(reference),
                    "name": String::from_utf8_lossy(name),
                    "upstream": (!upstream.is_empty()).then(|| String::from_utf8_lossy(upstream).into_owned()),
                    "upstream_ref": (!upstream_ref.is_empty()).then(|| String::from_utf8_lossy(upstream_ref).into_owned()),
                    "current": current,
                    "oid": String::from_utf8_lossy(oid),
                }));
            }
        }
        Ok(Value::Array(branches))
    }

    pub(crate) async fn git_directory(&self) -> Result<PathBuf> {
        let out = self
            .run(&args(&["rev-parse", "--absolute-git-dir"]))
            .await?;
        if out.code != 0 {
            bail!("{}", out.text().stderr);
        }
        Ok(PathBuf::from(os_from_bytes(
            out.stdout.strip_suffix(b"\n").unwrap_or(&out.stdout),
        )?))
    }
    pub(crate) async fn operation_state(&self) -> Result<Option<OperationState>> {
        let directory = self.git_directory().await?;
        for (marker, kind) in [
            ("rebase-merge", "rebase"),
            ("rebase-apply", "rebase"),
            ("MERGE_HEAD", "merge"),
            ("CHERRY_PICK_HEAD", "cherry_pick"),
            ("REVERT_HEAD", "revert"),
        ] {
            if directory.join(marker).exists() {
                let kind = if marker == "rebase-apply"
                    && directory.join(marker).join("applying").exists()
                {
                    "am"
                } else {
                    kind
                };
                let read_number = |name: &str| {
                    fs::read_to_string(directory.join(marker).join(name))
                        .ok()
                        .and_then(|s| s.trim().parse().ok())
                };
                return Ok(Some(OperationState {
                    kind: kind.into(),
                    step: read_number("msgnum").or_else(|| read_number("next")),
                    total: read_number("end").or_else(|| read_number("last")),
                }));
            }
        }
        if let Ok(todo) = fs::read_to_string(directory.join("sequencer/todo")) {
            let kind = if todo.lines().any(|line| line.starts_with("revert ")) {
                "revert"
            } else {
                "cherry_pick"
            };
            return Ok(Some(OperationState {
                kind: kind.into(),
                step: None,
                total: None,
            }));
        }
        Ok(None)
    }
    pub(crate) async fn tracking(
        &self,
        head: Option<&str>,
    ) -> Result<(Option<String>, Option<u64>, Option<u64>)> {
        let Some(head) = head else {
            return Ok((None, None, None));
        };
        let up = self
            .run(&args(&["rev-parse", "--symbolic-full-name", "@{upstream}"]))
            .await?;
        if up.code != 0 {
            return Ok((None, None, None));
        }
        let name = String::from_utf8(up.stdout)?.trim().to_owned();
        let tip = match self.resolve_commit(&name).await {
            Ok(tip) => tip,
            Err(_) => return Ok((Some(name), None, None)),
        };
        let counts = self
            .run(&args(&[
                "rev-list",
                "--left-right",
                "--count",
                &format!("{head}...{tip}"),
                "--",
            ]))
            .await?;
        if counts.code != 0 {
            bail!("{}", counts.text().stderr);
        }
        let text = std::str::from_utf8(&counts.stdout)?;
        let mut fields = text.split_whitespace();
        Ok((
            Some(name),
            Some(fields.next().context("missing ahead count")?.parse()?),
            Some(fields.next().context("missing behind count")?.parse()?),
        ))
    }
    pub(crate) async fn branch_list(&self) -> Result<Value> {
        let output = self
            .run(&args(&[
                "for-each-ref",
                "--sort=refname",
                "--format=%(refname)%00%(objectname)%00%(HEAD)%00%(upstream)%00%(worktreepath)%00%(symref)%00",
                "refs/heads",
                "refs/remotes",
            ]))
            .await?;
        if output.code != 0 {
            bail!("{}", output.text().stderr);
        }
        let mut fields = output.stdout.split(|b| *b == 0);
        let mut branches = Vec::new();
        while let Some(raw_ref) = fields.next() {
            let raw_ref = raw_ref.strip_prefix(b"\n").unwrap_or(raw_ref);
            if raw_ref.is_empty() {
                break;
            }
            let reference = String::from_utf8_lossy(raw_ref).into_owned();
            let oid =
                String::from_utf8_lossy(fields.next().context("missing branch oid")?).into_owned();
            let current = fields.next().context("missing current flag")? == b"*";
            let upstream = fields.next().context("missing upstream ref")?;
            let worktree = fields.next().context("missing worktree path")?;
            let symbolic = fields.next().context("missing symbolic branch target")?;
            // Match Desktop's branch enumeration: aliases such as remote HEAD
            // and symbolic local refs are not independent branch candidates.
            if !symbolic.is_empty() {
                continue;
            }
            let local = reference.starts_with("refs/heads/");
            let name = reference
                .strip_prefix(if local {
                    "refs/heads/"
                } else {
                    "refs/remotes/"
                })
                .unwrap_or(&reference);
            branches.push(json!({"name":name,"ref":reference,"oid":oid,"current":current,"local":local,"upstream":(!upstream.is_empty()).then(||String::from_utf8_lossy(upstream).into_owned()),"worktree":(!worktree.is_empty()).then(||String::from_utf8_lossy(worktree).into_owned())}));
        }
        Ok(Value::Array(branches))
    }
}

#[cfg(test)]
mod cherry_pick_state_tests {
    use super::*;
    use std::process::Command;

    #[tokio::test]
    async fn reads_last_fetch_for_each_linked_worktree() {
        let root =
            std::env::temp_dir().join(format!("desktop-last-fetch-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q", "-b", "main"]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-qm",
            "initial",
        ]);
        let linked = root.join("linked");
        git(&[
            "worktree",
            "add",
            "-qb",
            "linked-branch",
            linked.to_str().unwrap(),
        ]);
        let main = Repository::open(&root).await.unwrap();
        let child = Repository::open(&linked).await.unwrap();
        assert_eq!(
            main.renderer_last_fetch().await.unwrap()["kind"],
            "unavailable"
        );
        fs::write(root.join(".git/FETCH_HEAD"), "").unwrap();
        assert_eq!(main.renderer_last_fetch().await.unwrap()["kind"], "empty");
        fs::write(root.join(".git/FETCH_HEAD"), "data\n").unwrap();
        let main_value = main.renderer_last_fetch().await.unwrap();
        assert_eq!(main_value["kind"], "fetched");
        assert!(main_value["time"].as_i64().unwrap() > 0);
        assert_eq!(
            child.renderer_last_fetch().await.unwrap()["kind"],
            "unavailable"
        );
        fs::write(
            child.git_directory().await.unwrap().join("FETCH_HEAD"),
            "other\n",
        )
        .unwrap();
        assert_eq!(
            child.renderer_last_fetch().await.unwrap()["kind"],
            "fetched"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn reads_apply_backend_rebase_from_real_conflict() {
        let root =
            std::env::temp_dir().join(format!("desktop-apply-rebase-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let git = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap()
        };
        let run = |args: &[&str]| {
            let output = git(args);
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.name", "Test"]);
        run(&["config", "user.email", "test@example.invalid"]);
        fs::write(root.join("file"), "base\n").unwrap();
        run(&["add", "--", "file"]);
        run(&["-c", "commit.gpgsign=false", "commit", "-qm", "base"]);
        run(&["switch", "-qc", "topic"]);
        fs::write(root.join("file"), "topic\n").unwrap();
        run(&["-c", "commit.gpgsign=false", "commit", "-qam", "topic"]);
        let original = run(&["rev-parse", "HEAD"]);
        run(&["switch", "-q", "main"]);
        fs::write(root.join("file"), "main\n").unwrap();
        run(&["-c", "commit.gpgsign=false", "commit", "-qam", "main"]);
        let onto = run(&["rev-parse", "HEAD"]);
        run(&["switch", "-q", "topic"]);
        let rebase = git(&["rebase", "--apply", "main"]);
        assert!(!rebase.status.success(), "fixture must stop in conflict");

        let repository = Repository::open(&root).await.unwrap();
        let state = repository.renderer_rebase_state().await.unwrap();
        assert_eq!(state["head"], original);
        assert_eq!(
            state["progress"]["original"].as_str().unwrap().trim(),
            original
        );
        assert_eq!(
            state["progress"]["branch"].as_str().unwrap().trim(),
            "refs/heads/topic"
        );
        assert_eq!(state["progress"]["onto"].as_str().unwrap().trim(), onto);
        assert_eq!(state["progress"]["current"].as_str().unwrap().trim(), "1");
        assert_eq!(state["progress"]["total"].as_str().unwrap().trim(), "1");
        assert_eq!(
            repository.status_context().await.unwrap()["rebaseInternalState"],
            json!({
                "originalBranchTip":original,"targetBranch":"topic","baseBranchTip":onto
            })
        );
        // Displaying apply-backend progress must not authorize continuation:
        // the existing mutation certificate only covers the merge backend.
        fs::write(root.join("file"), "resolved\n").unwrap();
        run(&["add", "--", "file"]);
        let error = repository
            .renderer_rebase_continue_preview(&original)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Only merge-backend rebase continuation")
        );

        let apply = root.join(".git/rebase-apply");
        fs::write(apply.join("last"), vec![b'x'; 8193]).unwrap();
        assert!(repository.renderer_rebase_state().await.is_err());
        fs::remove_file(apply.join("last")).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join(".git/config"), apply.join("last")).unwrap();
            assert!(repository.renderer_rebase_state().await.is_err());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn reads_rebase_state_and_rejects_unsafe_progress() {
        let root =
            std::env::temp_dir().join(format!("desktop-rebase-state-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        let repo = Repository::open(&root).await.unwrap();
        assert_eq!(repo.renderer_rebase_state().await.unwrap(), Value::Null);
        let git_dir = root.join(".git");
        let head = "a".repeat(40);
        fs::write(git_dir.join("REBASE_HEAD"), format!("{head}\n")).unwrap();
        assert_eq!(repo.renderer_rebase_state().await.unwrap(), Value::Null);
        assert!(repo.status_context().await.unwrap()["rebaseInternalState"].is_null());
        fs::create_dir(git_dir.join("rebase-apply")).unwrap();
        assert_eq!(
            repo.renderer_rebase_state().await.unwrap(),
            json!({"head":head,"backend":"apply","progress":null})
        );
        fs::write(git_dir.join("rebase-apply/applying"), "1\n").unwrap();
        assert_eq!(repo.renderer_rebase_state().await.unwrap(), Value::Null);
        fs::remove_dir_all(git_dir.join("rebase-apply")).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&git_dir, git_dir.join("rebase-apply")).unwrap();
            assert!(repo.renderer_rebase_state().await.is_err());
            fs::remove_file(git_dir.join("rebase-apply")).unwrap();
        }
        let rebase = git_dir.join("rebase-merge");
        fs::create_dir(&rebase).unwrap();
        fs::create_dir(git_dir.join("rebase-apply")).unwrap();
        assert!(repo.renderer_rebase_state().await.is_err());
        fs::remove_dir(git_dir.join("rebase-apply")).unwrap();
        for (name, content) in [
            ("orig-head", format!("{head}\n")),
            ("head-name", "refs/heads/topic\n".into()),
            ("onto", format!("{head}\n")),
            ("msgnum", "2\n".into()),
            ("end", "4\n".into()),
        ] {
            fs::write(rebase.join(name), content).unwrap();
        }
        let state = repo.renderer_rebase_state().await.unwrap();
        assert_eq!(state["progress"]["current"], "2\n");
        assert_eq!(state["progress"]["total"], "4\n");
        assert_eq!(
            repo.status_context().await.unwrap()["rebaseInternalState"],
            json!({"originalBranchTip":head,"targetBranch":"topic","baseBranchTip":head})
        );
        fs::remove_file(rebase.join("head-name")).unwrap();
        assert!(repo.renderer_rebase_state().await.unwrap()["progress"]["branch"].is_null());
        fs::write(rebase.join("end"), vec![b'x'; 8193]).unwrap();
        assert!(repo.renderer_rebase_state().await.is_err());
        #[cfg(unix)]
        {
            fs::remove_file(rebase.join("end")).unwrap();
            std::os::unix::fs::symlink(git_dir.join("config"), rebase.join("end")).unwrap();
            assert!(repo.renderer_rebase_state().await.is_err());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn reads_bounded_sequencer_without_following_links() {
        let root =
            std::env::temp_dir().join(format!("desktop-pick-state-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        let repo = Repository::open(&root).await.unwrap();
        assert_eq!(
            repo.renderer_cherry_pick_state().await.unwrap(),
            Value::Null
        );
        let git_dir = root.join(".git");
        let hash = "a".repeat(40);
        fs::write(git_dir.join("CHERRY_PICK_HEAD"), format!("{hash}\n")).unwrap();
        assert_eq!(
            repo.renderer_cherry_pick_state().await.unwrap(),
            json!({"head":hash,"sequence":null})
        );
        fs::create_dir(git_dir.join("sequencer")).unwrap();
        fs::write(git_dir.join("sequencer/head"), format!("{hash}\n")).unwrap();
        fs::write(git_dir.join("sequencer/abort-safety"), format!("{hash}\n")).unwrap();
        fs::write(
            git_dir.join("sequencer/todo"),
            "pick abc1234 first\npick def5678 second\n",
        )
        .unwrap();
        let value = repo.renderer_cherry_pick_state().await.unwrap();
        assert_eq!(value["sequence"]["head"], format!("{hash}\n"));
        assert_eq!(
            value["sequence"]["todo"],
            "pick abc1234 first\npick def5678 second\n"
        );
        fs::write(git_dir.join("sequencer/todo"), vec![b'x'; 1024 * 1024 + 1]).unwrap();
        assert!(repo.renderer_cherry_pick_state().await.is_err());
        #[cfg(unix)]
        {
            fs::remove_file(git_dir.join("sequencer/todo")).unwrap();
            std::os::unix::fs::symlink(git_dir.join("config"), git_dir.join("sequencer/todo"))
                .unwrap();
            assert!(repo.renderer_cherry_pick_state().await.is_err());
        }
        fs::remove_dir_all(root).unwrap();
    }
}
