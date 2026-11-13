use crate::CreationFile;
use serde::{Deserialize, Serialize};

/// A literal repository-relative path, preserving Unix filename bytes when needed.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum GitPath {
    Text(String),
    Bytes { bytes: Vec<u8> },
}

/// Source of the changes summarized by Copilot.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopilotDiffSource {
    #[default]
    Staged,
    WorkingTree,
}

/// Baseline used by the renderer's working-tree diff.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RendererDiffBase {
    Head,
    Index,
    Empty,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitTrailer {
    pub token: String,
    pub value: String,
}

/// A reviewed HEAD-to-working-tree line selection.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CopilotLineSelection {
    pub path: GitPath,
    #[serde(default)]
    pub original_path: Option<GitPath>,
    pub basis: String,
    pub lines: Vec<usize>,
}

/// A selected GitHub identity; credentials remain in GitHub CLI storage.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopilotAccount {
    pub host: String,
    pub login: String,
    pub id: u64,
}

/// Branch and commit retained with a restored selection.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CopilotExpectedHead {
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub sha: Option<String>,
}

/// Static custom-provider settings. Credentials must not appear in Debug output.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CopilotProvider {
    #[serde(rename = "type")]
    pub provider_type: String,
    pub base_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wire_api: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bearer_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub azure: Option<CopilotProviderAzure>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CopilotProviderAzure {
    pub api_version: String,
}

impl std::fmt::Debug for CopilotProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CopilotProvider([redacted])")
    }
}

/// Inputs explicitly reviewed before publishing a local repository.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublishPreviewRequest {
    pub host: String,
    pub target: crate::AccountTarget,
    pub owner: String,
    pub name: String,
    pub description: String,
    pub private: bool,
}

/// Typed operations shared by the CLI, browser transport, and native shell.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    LfsTransfer {
        request: crate::LfsTransferRequest,
    },
    LfsCheckout {
        #[serde(default)]
        patterns: Vec<GitPath>,
    },
    LfsTrack {
        patterns: Vec<String>,
        #[serde(default)]
        filename: bool,
        lockable: Option<bool>,
    },
    LfsUntrack {
        patterns: Vec<String>,
    },
    LfsToolStatus,
    LfsStatus {
        #[serde(default)]
        paths: Vec<GitPath>,
    },
    LfsFileTracking {
        paths: Vec<GitPath>,
    },
    LfsInstall {
        #[serde(default)]
        scope: crate::LfsInstallScope,
        #[serde(default)]
        force: bool,
        #[serde(default)]
        skip_repo: bool,
        #[serde(default)]
        skip_smudge: bool,
    },
    Github {
        request: crate::GitHubRequest,
    },
    History {
        revision: Option<String>,
        #[serde(default)]
        offset: usize,
        #[serde(default = "history_limit")]
        limit: u16,
    },
    CommitFiles {
        commit: String,
        #[serde(default)]
        parent: usize,
    },
    CommitDiff {
        path: GitPath,
        commit: String,
        #[serde(default)]
        parent: usize,
    },
    BranchList,
    /// Renderer operation markers and the interactive rebase's original refs.
    StatusContext,
    RendererCherryPickState,
    RendererRebaseState,
    RendererFinishMergeCommit,
    RendererAbortSquash {
        expected_head: String,
        expected_branch: String,
    },
    StatusConflictDetails {
        kind: StatusConflictKind,
    },
    /// Git's disambiguated names for renderer branch enumeration.
    BranchEnumeration {
        #[serde(default)]
        prefixes: Vec<String>,
    },
    BranchMerged {
        branch: String,
    },
    BranchPointsAt {
        commitish: String,
    },
    AheadBehind {
        range: String,
    },
    RevisionList {
        range: String,
    },
    HasMergeCommits {
        range: String,
    },
    BranchPolicy,
    BranchNameRules {
        name: String,
    },
    UpstreamSetupPreview,
    SetupUpstream {
        basis: Option<String>,
    },
    PublishContext {
        host: String,
    },
    PublishOrganizations {
        host: String,
        target: crate::AccountTarget,
    },
    PublishPreview {
        request: PublishPreviewRequest,
    },
    PublishCreate {
        request: PublishPreviewRequest,
        basis: String,
        acknowledge_partial: bool,
    },
    PublishContinuePreview {
        receipt_id: String,
    },
    PublishContinue {
        receipt_id: String,
        basis: String,
        acknowledge_partial: bool,
    },
    PublishHistory {},
    AcknowledgePublish {
        receipt_id: String,
    },
    RepositoryAccount,
    RepositoryAccountCatalog,
    RepositoryAccountPreview {
        target: crate::AccountTarget,
    },
    SetRepositoryAccount {
        target: crate::AccountTarget,
        basis: String,
    },
    RepositoryContext,
    RefreshRepositoryContext,
    SetForkContributionTarget {
        target: crate::ForkContributionTarget,
        basis: Option<String>,
    },
    RemoteDeletionTargets,
    RemoteBranchDeletePreview {
        request: crate::RemoteBranchDeletion,
    },
    DeleteRemoteBranch {
        request: crate::RemoteBranchDeletion,
        basis: Option<String>,
    },
    RemoteDeletionHistory,
    RestoreRemoteTip {
        receipt_id: String,
        endpoint: usize,
        name: String,
    },
    ForgetRemoteDeletion {
        receipt_id: String,
    },
    ConflictDetail {
        path: GitPath,
    },
    StageConflict {
        path: GitPath,
        basis: String,
        #[serde(default)]
        allow_markers: bool,
    },
    ResolveConflict {
        path: GitPath,
        choice: ConflictChoice,
        basis: String,
    },
    Recover {
        kind: RecoveryKind,
        action: RecoveryAction,
    },
    RendererCherryPickContinuePreview {
        expected: String,
    },
    RendererCherryPickStartPreview {
        commits: Vec<String>,
        expected_head: String,
        expected_branch: Option<String>,
    },
    RendererCherryPickStart {
        commits: Vec<String>,
        expected_head: String,
        expected_branch: Option<String>,
        basis: String,
    },
    RendererCherryPickContinue {
        expected: String,
        basis: String,
        empty: bool,
    },
    RendererRebaseContinuePreview {
        expected: String,
    },
    RendererRebaseStartPreview {
        request: RendererRebaseStartRequest,
    },
    RendererMergeStartPreview {
        request: RendererMergeStartRequest,
    },
    RendererMergeStart {
        request: RendererMergeStartRequest,
        basis: String,
    },
    RendererMixedResetPreview {
        request: RendererMixedResetRequest,
    },
    RendererMixedReset {
        request: RendererMixedResetRequest,
        basis: String,
    },
    RendererFastForwardPreview {
        request: RendererFastForwardRequest,
    },
    RendererFastForward {
        request: RendererFastForwardRequest,
        basis: String,
    },
    RendererFetchPreview {
        request: RendererFetchRequest,
    },
    RendererFetch {
        request: RendererFetchRequest,
        basis: String,
    },
    RendererRemoteHeadPreview {
        request: RendererFetchRequest,
    },
    RendererRemoteHead {
        request: RendererFetchRequest,
        basis: String,
    },
    RendererRebaseStart {
        request: RendererRebaseStartRequest,
        basis: String,
    },
    RendererRebaseContinue {
        expected: String,
        basis: String,
        empty: bool,
        no_verify: bool,
    },
    RenameBranchPreview {
        name: String,
        new_name: String,
    },
    BranchRenameHistory,
    AcknowledgeBranchRename {
        receipt_id: String,
    },
    RenameBranch {
        name: String,
        new_name: String,
        basis: Option<String>,
    },
    TrackBranch {
        name: String,
        upstream: String,
    },
    Detach {
        commit: String,
    },
    GitSettings {
        #[serde(default)]
        scope: crate::ConfigScope,
    },
    GitIdentityValue {
        key: String,
        #[serde(default)]
        only_local: bool,
    },
    GitEffectiveIdentity {
        role: GitIdentityRole,
    },
    GitDescription,
    IndexChanges,
    RendererCommit {
        revision: String,
    },
    RendererAuthors {
        revisions: Vec<String>,
    },
    RendererHistory {
        revision: Option<String>,
        limit: Option<u64>,
        skip: Option<u64>,
        #[serde(default)]
        first_parent: bool,
        #[serde(default)]
        exclude_remotes: bool,
    },
    RendererRecentBranches,
    RendererBranchCheckouts {
        after_date: String,
    },
    RendererStashLog,
    RendererTagRefs,
    RendererCommitChanges {
        revision: String,
    },
    RendererLargeFiles {
        paths: Vec<GitPath>,
    },
    MergeBase {
        first: String,
        second: String,
    },
    MergeTreePreview {
        ours: String,
        theirs: String,
    },
    InterpretTrailers {
        message: String,
        #[serde(default)]
        parse: bool,
        #[serde(default)]
        unfold: bool,
        #[serde(default)]
        trailers: Vec<CommitTrailer>,
    },
    RendererCommitDiff {
        revision: String,
        path: GitPath,
        old_path: Option<GitPath>,
        #[serde(default)]
        hide_whitespace: bool,
        #[serde(default)]
        snapshot: bool,
    },
    RendererCompareDiff {
        before: String,
        after: String,
        merge_base: bool,
        path: GitPath,
        old_path: Option<GitPath>,
        #[serde(default)]
        hide_whitespace: bool,
        #[serde(default)]
        snapshot: bool,
    },
    RendererCompareChanges {
        before: String,
        after: String,
        merge_base: bool,
    },
    RendererResolutionBegin,
    RendererResolutionAppend {
        snapshot_id: String,
        offset: u64,
        data: String,
    },
    RendererResolutionDiff {
        snapshot_id: String,
        size: u64,
        path: GitPath,
        stage: Option<u8>,
        hide_whitespace: bool,
    },
    RendererCompareDiffChunk {
        snapshot_id: String,
        offset: u64,
        length: u32,
    },
    RendererCompareDiffRelease {
        snapshot_id: String,
    },
    RendererStagedDiff {
        amend_head: Option<String>,
    },
    RendererWorkingDiff {
        path: GitPath,
        base: RendererDiffBase,
        #[serde(default)]
        hide_whitespace: bool,
        #[serde(default)]
        no_textconv: bool,
    },
    RendererWorkingFile { path: GitPath },
    RendererBlobPrefix {
        revision: String,
        path: GitPath,
        length: u32,
    },
    RendererBlobSnapshot {
        revision: String,
        path: GitPath,
    },
    RendererBlobChunk {
        revision: String,
        path: GitPath,
        offset: u64,
        length: u32,
        expected_oid: Option<String>,
    },
    SafeFetchRemotes,
    ManageRemote {
        action: crate::ManagedRemoteAction,
    },
    SetGitDescription {
        description: String,
    },
    WriteCreationFile {
        kind: CreationFile,
        contents: String,
    },
    GitIgnore {
        action: crate::GitIgnoreAction,
    },
    GitReadSetting {
        key: crate::GitReadSettingKey,
        #[serde(default)]
        only_local: bool,
        #[serde(default)]
        canonical_bool: bool,
    },
    SetGitSetting {
        #[serde(default)]
        scope: crate::ConfigScope,
        key: String,
        value: Option<String>,
    },
    Worktrees,
    WorktreeAdd {
        path: GitPath,
        branch: String,
        #[serde(default)]
        new_branch: bool,
        start: Option<String>,
    },
    RendererWorktreePreview { action: crate::renderer_worktree::WorktreeAction },
    RendererWorktreeApply { action: crate::renderer_worktree::WorktreeAction, basis: String },
    WorktreeRemove {
        path: GitPath,
        #[serde(default)]
        force: bool,
    },
    WorktreeMove {
        from: GitPath,
        to: GitPath,
    },
    WorktreeLock {
        path: GitPath,
        locked: bool,
        reason: Option<String>,
    },
    WorktreePrune {
        #[serde(default = "default_true")]
        dry_run: bool,
    },
    Submodules,
    RendererCheckoutIndex {
        paths: Vec<GitPath>,
    },
    RendererCheckoutHeadPaths {
        paths: Vec<GitPath>,
    },
    RendererResetIndex {
        paths: Vec<GitPath>,
    },
    RendererUpdateIndex {
        paths: Vec<GitPath>,
        force_remove: bool,
    },
    RendererUnstageAll,
    RendererRemoveIndexAll,
    RendererDeleteInitialHead {
        expected_oid: String,
    },
    VerifySelectedIndex {
        tree: String,
        head: Option<String>,
        branch: Option<String>,
    },
    RendererSubmoduleStatus,
    RendererResetSubmodules {
        paths: Vec<GitPath>,
    },
    RendererUpdateSubmodules {
        allow_file_protocol: bool,
    },
    SubmoduleDisplayUrl {
        path: GitPath,
    },
    SubmoduleUpdate {
        #[serde(default)]
        paths: Vec<GitPath>,
        #[serde(default = "default_true")]
        recursive: bool,
    },
    SubmoduleSync {
        #[serde(default)]
        paths: Vec<GitPath>,
        #[serde(default = "default_true")]
        recursive: bool,
    },
    SubmoduleDeinit {
        paths: Vec<GitPath>,
        #[serde(default)]
        force: bool,
    },
    SubmoduleAdd {
        url: String,
        path: GitPath,
        name: Option<String>,
    },
    Status,
    /// Porcelain-v2 records for the desktop status view, preserving path bytes.
    StatusDetailed {
        #[serde(default = "default_true")]
        include_untracked: bool,
    },
    Snapshot,
    Log,
    Branches,
    Diff {
        #[serde(default)]
        staged: bool,
        path: Option<GitPath>,
    },
    DiffDetail {
        path: GitPath,
        #[serde(default)]
        staged: bool,
    },
    SelectDiff {
        path: GitPath,
        #[serde(default)]
        staged: bool,
        basis: String,
        lines: Vec<usize>,
    },
    SelectRenamedDiff {
        path: GitPath,
        original_path: GitPath,
        basis: String,
        lines: Vec<usize>,
        #[serde(default)]
        preserve_source: bool,
    },
    DiscardPreview {
        path: GitPath,
        #[serde(default)]
        scope: DiscardScope,
        selection: Option<DiscardSelection>,
    },
    Discard {
        path: GitPath,
        #[serde(default)]
        scope: DiscardScope,
        selection: Option<DiscardSelection>,
        basis: String,
    },
    DiscardBatchPreview {
        paths: Vec<GitPath>,
    },
    DiscardBatch {
        paths: Vec<GitPath>,
        basis: String,
    },
    ListDiscards,
    ForgetDiscard {
        undo_id: String,
    },
    UndoDiscard {
        undo_id: String,
    },
    Stage {
        paths: Vec<GitPath>,
    },
    Unstage {
        paths: Vec<GitPath>,
    },
    CommitPreview,
    SelectedCommitPreview {
        #[serde(default)]
        paths: Vec<GitPath>,
        #[serde(default)]
        remove_paths: Vec<GitPath>,
        #[serde(default)]
        partial_selections: Vec<CopilotLineSelection>,
        #[serde(default)]
        allow_empty: bool,
        amend_head: Option<String>,
        expected_head: Option<CopilotExpectedHead>,
    },
    PrepareSelectedCommit {
        #[serde(default)]
        paths: Vec<GitPath>,
        #[serde(default)]
        remove_paths: Vec<GitPath>,
        #[serde(default)]
        partial_selections: Vec<CopilotLineSelection>,
        #[serde(default)]
        allow_empty: bool,
        amend_head: Option<String>,
        expected_head: Option<CopilotExpectedHead>,
        basis: String,
    },
    PublishSelectedCommit {
        checkpoint_id: String,
        message: String,
        #[serde(default)]
        no_verify: bool,
        #[serde(default)]
        sign_off: bool,
        #[serde(default)]
        allow_large_files: bool,
    },
    ListCommitCheckpoints,
    InspectCommitCheckpoint {
        checkpoint_id: String,
    },
    RevalidateCommitCheckpoint {
        checkpoint_id: String,
    },
    RecoverPublishedCommitCheckpoint {
        checkpoint_id: String,
    },
    CloseUnchangedCommitCheckpoint {
        checkpoint_id: String,
    },
    ForgetCommitCheckpoint {
        checkpoint_id: String,
    },
    InitialCommit {
        paths: Vec<GitPath>,
    },
    Commit {
        #[serde(default)]
        allow_large_files: bool,
        basis: Option<String>,
        summary: String,
        #[serde(default)]
        description: String,
    },
    CheckoutPreview {
        branch: String,
    },
    RendererCheckoutDirect {
        branch: String,
        basis: String,
        #[serde(default)]
        allow_file_protocol: bool,
    },
    RendererDetachPreview {
        commit: String,
    },
    RendererDetach {
        commit: String,
        basis: String,
        #[serde(default)]
        allow_file_protocol: bool,
    },
    CheckoutHistory,
    AcknowledgeCheckout {
        receipt_id: String,
    },
    Checkout {
        branch: String,
        #[serde(default)]
        strategy: ChangesStrategy,
        basis: Option<String>,
        #[serde(default)]
        replace_stash: bool,
        #[serde(default)]
        restore_index: bool,
    },
    CreateBranchPreview {
        name: String,
        start_point: Option<String>,
        #[serde(default)]
        no_track: bool,
        #[serde(default)]
        checkout: bool,
    },
    CreateBranch {
        name: String,
        start_point: Option<String>,
        #[serde(default)]
        no_track: bool,
        #[serde(default)]
        checkout: bool,
        #[serde(default)]
        strategy: ChangesStrategy,
        basis: Option<String>,
        #[serde(default)]
        replace_stash: bool,
        #[serde(default)]
        restore_index: bool,
    },
    DeleteBranchPreview {
        name: String,
        to_checkout: Option<String>,
        upstream: Option<crate::UpstreamDeletion>,
    },
    DeleteBranch {
        name: String,
        to_checkout: Option<String>,
        #[serde(default)]
        ref_only: bool,
        #[serde(default)]
        force: bool,
        #[serde(default)]
        strategy: ChangesStrategy,
        basis: Option<String>,
        #[serde(default)]
        replace_stash: bool,
        #[serde(default)]
        restore_index: bool,
        upstream: Option<crate::UpstreamDeletion>,
    },
    DeletedBranches,
    RestoreDeletedBranch {
        receipt_id: String,
        name: Option<String>,
    },
    ForgetDeletedBranch {
        receipt_id: String,
    },
    Fetch {
        remote: String,
    },
    Pull {
        remote: String,
        branch: String,
    },
    Push {
        remote: String,
        branch: String,
        #[serde(default)]
        set_upstream: bool,
    },
    Merge {
        branch: String,
    },
    MergeAbort,
    Rebase {
        branch: String,
    },
    RebaseAbort,
    RebaseContinue,
    CherryPick {
        commit: String,
        mainline: Option<u32>,
    },
    Revert {
        commit: String,
        mainline: Option<u32>,
    },
    RendererRevertPreview {
        commit: String,
    },
    RendererRevert {
        commit: String,
        basis: String,
    },
    StashSave {
        #[serde(default)]
        message: String,
        #[serde(default)]
        include_untracked: bool,
        #[serde(default)]
        desktop: bool,
    },
    StashList,
    RendererStashChanges {
        oid: String,
    },
    RendererStashSave {
        branch: String,
    },
    RendererStashPop {
        oid: String,
        index: u32,
        basis: String,
    },
    RendererStashDrop {
        oid: String,
        index: u32,
        basis: String,
    },
    StashFiles {
        oid: String,
    },
    StashApply {
        oid: String,
        index: Option<u32>,
        basis: Option<String>,
        #[serde(default)]
        restore_index: bool,
        #[serde(default)]
        pop: bool,
    },
    StashDrop {
        oid: String,
        index: u32,
        basis: String,
    },
    StashMove {
        oid: String,
        index: u32,
        basis: String,
        branch: String,
    },
    Tags,
    TagCreate {
        name: String,
        target: Option<String>,
    },
    TagCreateAnnotated {
        name: String,
        target: String,
    },
    TagDelete {
        name: String,
    },
    Remotes,
    AddRemote {
        name: String,
        url: String,
    },
    RemoveRemote {
        name: String,
    },
    CopilotProbe {
        #[serde(default)]
        runtime_root: Option<String>,
    },
    CopilotCreateSession {
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        reasoning_effort: Option<String>,
        #[serde(default)]
        account: Option<CopilotAccount>,
    },
    CopilotSendMessage {
        session_id: String,
        message: String,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    CopilotCloseSession {
        session_id: String,
    },
    CopilotCommitDiffDetail {
        path: GitPath,
        #[serde(default)]
        original_path: Option<GitPath>,
    },
    CopilotGenerateCommitMessage {
        #[serde(default)]
        paths: Vec<GitPath>,
        #[serde(default)]
        remove_paths: Vec<GitPath>,
        #[serde(default)]
        system_prompt: Option<String>,
        #[serde(default)]
        source: CopilotDiffSource,
        #[serde(default)]
        partial_selections: Vec<CopilotLineSelection>,
        #[serde(default)]
        amend_head: Option<String>,
        #[serde(default)]
        rule_descriptions: Vec<String>,
        #[serde(default)]
        account: Option<Box<CopilotAccount>>,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        reasoning_effort: Option<String>,
        #[serde(default)]
        timeout_ms: Option<u64>,
        #[serde(default)]
        provider: Option<Box<CopilotProvider>>,
        #[serde(default)]
        expected_head: Option<Box<CopilotExpectedHead>>,
    },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusConflictKind {
    Merge,
    Rebase,
    Working,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitIdentityRole {
    Author,
    Committer,
}

#[derive(Debug, Serialize)]
pub struct CommandOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Serialize)]
pub struct FileChange {
    /// Display only; operations should send `path`, which preserves exact bytes.
    pub display_path: String,
    pub path: GitPath,
    pub original_path: Option<GitPath>,
    pub index: char,
    pub worktree: char,
    pub conflicted: bool,
    pub size: Option<u64>,
    pub modified_ns: Option<u128>,
}

/// One porcelain-v2 record. Paths and branch header values retain Git's bytes.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DetailedStatusRecord {
    Header {
        value: GitPath,
    },
    Entry {
        entry_kind: char,
        status_code: String,
        submodule_status_code: String,
        path: GitPath,
        old_path: Option<GitPath>,
        rename_or_copy_score: Option<u16>,
    },
}

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub root: String,
    pub root_id: String,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub files: Vec<FileChange>,
    pub branch_policy: Option<crate::BranchPermissions>,
    pub default_branch: Option<crate::DefaultBranch>,
    pub repository_context: crate::RepositoryContext,
    pub repository_account: crate::RepositoryAccount,
    pub upstream: Option<String>,
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
    pub operation: Option<crate::OperationState>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RendererRebaseStartRequest {
    pub base_ref: String,
    pub base_head: String,
    pub target_ref: String,
    pub target_head: String,
    pub current_head: String,
    pub current_branch: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RendererMergeStartRequest {
    pub source_ref: String,
    pub source_head: String,
    pub current_ref: String,
    pub current_head: String,
    pub squash: bool,
    pub no_verify: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RendererMixedResetRequest {
    pub target: String,
    pub head: String,
    pub branch: Option<String>,
    pub undo: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RendererFastForwardRequest {
    pub branches: Vec<RendererTrackingBranch>,
    pub recurse_submodules: bool,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RendererFetchRequest {
    pub remote: String,
    pub expected_url: String,
    pub recurse_submodules: bool,
    #[serde(default)]
    pub is_background_task: bool,
    #[serde(default)]
    pub report_progress: bool,
    #[serde(default)]
    pub expected_saml_owner: Option<String>,
    #[serde(default)]
    pub refspec: Option<String>,
}

impl std::fmt::Debug for RendererFetchRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RendererFetchRequest")
            .field("remote", &self.remote)
            .field("expected_url", &"<redacted>")
            .field("recurse_submodules", &self.recurse_submodules)
            .field("is_background_task", &self.is_background_task)
            .field("report_progress", &self.report_progress)
            .field("expected_saml_owner", &self.expected_saml_owner)
            .field("refspec", &self.refspec.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RendererTrackingBranch {
    #[serde(rename = "ref")]
    pub reference: String,
    pub sha: String,
    pub upstream_ref: String,
    pub upstream_sha: String,
}

fn history_limit() -> u16 {
    30
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictChoice {
    Ours,
    Theirs,
    Delete,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryKind {
    Merge,
    Rebase,
    CherryPick,
    Revert,
    Am,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAction {
    Continue,
    Abort,
    Skip,
    Quit,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscardScope {
    #[default]
    AllChanges,
    WorkingTree,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiscardSelection {
    pub basis: String,
    pub lines: Vec<usize>,
}

/// Values match the shared upstream UncommittedChangesStrategy model.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum ChangesStrategy {
    #[default]
    AskForConfirmation,
    StashOnCurrentBranch,
    MoveToNewBranch,
}
