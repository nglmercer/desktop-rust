//! Read Git symbolic refs without invoking a renderer-side process.
use crate::{args, process};
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::path::Path;

pub async fn symbolic_ref(path: &Path, reference: &str) -> Result<Value> {
    anyhow::ensure!(path.is_absolute(), "repository path must be absolute");
    anyhow::ensure!(
        reference.len() <= 4096 && !reference.contains('\0') && !reference.starts_with('-'),
        "Invalid symbolic ref"
    );
    let output = process::run(path, &args(&["symbolic-ref", "-q", "--", reference])).await?;
    if matches!(output.code, 1 | 128) {
        return Ok(Value::Null);
    }
    if output.code != 0 {
        bail!("Unable to read symbolic ref");
    }
    anyhow::ensure!(output.stdout.len() <= 65536, "Symbolic ref exceeds 64 KiB");
    Ok(json!(String::from_utf8_lossy(&output.stdout).trim()))
}
