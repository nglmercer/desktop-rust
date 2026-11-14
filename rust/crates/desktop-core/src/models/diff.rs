//! Port of `app/src/models/diff/*` and the shape returned by
//! `app/src/lib/diff-parser.ts`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiffType {
    Text,
    Image,
    Binary,
    Submodule,
    LargeText,
    Unrenderable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiffLineType {
    Context,
    Add,
    Delete,
    Hunk,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub text: String,
    #[serde(rename = "type")]
    pub kind: DiffLineType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_line_number: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_line_number: Option<u32>,
    #[serde(default)]
    pub no_trailing_new_line: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffHunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
    pub old_start_line: u32,
    pub old_line_count: u32,
    pub new_start_line: u32,
    pub new_line_count: u32,
    #[serde(default)]
    pub expansion_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diff {
    #[serde(rename = "kind")]
    pub diff_type: DiffType,
    #[serde(default)]
    pub hunks: Vec<DiffHunk>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub new_path: String,
    #[serde(default)]
    pub is_binary: bool,
    #[serde(default)]
    pub text: Option<String>,
}

impl Diff {
    pub fn text(path: impl Into<String>, hunks: Vec<DiffHunk>) -> Self {
        Self {
            diff_type: DiffType::Text,
            hunks,
            old_path: None,
            new_path: path.into(),
            is_binary: false,
            text: None,
        }
    }
    pub fn binary(path: impl Into<String>) -> Self {
        Self {
            diff_type: DiffType::Binary,
            hunks: Vec::new(),
            old_path: None,
            new_path: path.into(),
            is_binary: true,
            text: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Image {
    pub media_type: String,
    pub bytes: String, // base64, as the legacy shell carried it over the bridge
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageDiffType {
    #[default]
    TwoUp,
    Swipe,
    OnionSkin,
    Difference,
}

/// `DiffSelection` — which lines are staged for a partial commit.
/// `None`/`All` are the compact representations; `Partial` carries explicit rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DiffSelection {
    None,
    All,
    Partial { included: Vec<u32> },
}
