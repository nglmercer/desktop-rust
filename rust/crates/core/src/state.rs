//! Branch tracking and in-progress Git operations, including linked worktrees.
use crate::{Repository, args, os_from_bytes};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

#[derive(Debug, Serialize)]
pub struct OperationState {
    pub kind: String,
    pub step: Option<u64>,
    pub total: Option<u64>,
}
impl Repository {
    pub(crate) async fn status_context(&self) -> Result<Value> {
        let directory = self.git_directory().await?;
        let rebase = if directory.join("REBASE_HEAD").exists() {
            let read = |name: &str| -> Option<String> {
                use std::io::Read;
                let path = directory.join("rebase-merge").join(name);
                let mut options = fs::OpenOptions::new();
                options.read(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.custom_flags(libc::O_NONBLOCK);
                }
                let file = options.open(path).ok()?;
                let metadata = file.metadata().ok()?;
                if !metadata.is_file() || metadata.len() > 8192 {
                    return None;
                }
                let mut bytes = Vec::new();
                file.take(8193).read_to_end(&mut bytes).ok()?;
                if bytes.len() > 8192 {
                    return None;
                }
                Some(String::from_utf8_lossy(&bytes).into_owned())
            };
            match (read("orig-head"), read("head-name"), read("onto")) {
                (Some(original), Some(branch), Some(base)) => {
                    let branch = branch
                        .strip_prefix("refs/heads/")
                        .map_or(branch.clone(), |name| name.trim().to_owned());
                    json!({"originalBranchTip":original.trim(), "targetBranch":branch, "baseBranchTip":base.trim()})
                }
                _ => Value::Null,
            }
        } else {
            Value::Null
        };
        Ok(json!({
            "mergeHeadFound":directory.join("MERGE_HEAD").exists(),
            "squashMsgFound":directory.join("SQUASH_MSG").exists(),
            "isCherryPickingHeadFound":directory.join("CHERRY_PICK_HEAD").exists(),
            "rebaseInternalState":rebase,
        }))
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
