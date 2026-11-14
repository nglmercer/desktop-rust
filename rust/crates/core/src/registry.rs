//! Persistent repository selection, independent of Electron and browser storage.
use crate::{Repository, accounts::StoredAccount, args, process};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub struct Registry {
    path: PathBuf,
    // Exact descriptor paths remain valid in cloned registries used by
    // detached blocking operations after the caller is cancelled.
    owned_handles: Option<std::sync::Arc<Vec<std::sync::Arc<fs::File>>>>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SavedRepositories {
    pub version: u32,
    #[serde(with = "crate::paths::optional")]
    pub selected: Option<PathBuf>,
    #[serde(with = "crate::paths::list")]
    pub repositories: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub records: Vec<SavedRepositoryRecord>,
    /// Authoritative publication intents survive removal of a saved record.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) publication_intents: Vec<PublicationIntent>,
    /// Private write-ahead records for selected-account clone publication.
    #[serde(skip)]
    pub(crate) clone_intents: Vec<CloneIntent>,
}

/// Open each private sidecar once, without following its final component, and
/// enforce the bound on that same descriptor even if the pathname is swapped.
fn open_private_sidecar(path: &Path) -> Result<Option<fs::File>> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_OPEN_REPARSE_POINT: inspect the link itself, never its target.
        options.custom_flags(0x0020_0000);
    }
    match options.open(path) {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => bail!("Cannot safely inspect private registry metadata"),
    }
}

fn read_private_descriptor(file: &fs::File, limit: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let before = file.metadata()?;
    anyhow::ensure!(
        before.is_file() && before.len() <= limit,
        "Invalid private registry metadata"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(before.nlink() == 1, "Invalid private registry metadata");
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        anyhow::ensure!(
            before.file_attributes() & 0x400 == 0,
            "Invalid private registry metadata"
        );
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    anyhow::ensure!(
        bytes.len() as u64 == before.len()
            && after.len() == before.len()
            && after.modified().ok() == before.modified().ok(),
        "Private registry metadata changed while reading"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(
            before.dev() == after.dev()
                && before.ino() == after.ino()
                && after.nlink() == 1
                && before.ctime() == after.ctime()
                && before.ctime_nsec() == after.ctime_nsec(),
            "Private registry metadata changed while reading"
        );
    }
    Ok(bytes)
}

fn read_private_sidecar(path: &Path, limit: u64) -> Result<Option<Vec<u8>>> {
    open_private_sidecar(path)?
        .map(|file| read_private_descriptor(&file, limit))
        .transpose()
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CloneIntent {
    pub(crate) id: String,
    pub(crate) phase: String,
    pub(crate) url: String,
    pub(crate) certificate: Option<String>,
    #[serde(with = "record_path")]
    pub(crate) stage: PathBuf,
    #[serde(with = "record_path")]
    pub(crate) destination: PathBuf,
    pub(crate) device: u64,
    pub(crate) inode: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) parent_identity: Option<(u64, u64)>,
    pub(crate) account: StoredAccount,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PublicationIntent {
    pub id: String,
    pub repository_id: String,
    /// Canonical common Git directory links related worktrees.
    #[serde(default)]
    pub common_id: String,
    pub record_id: String,
    pub phase: String,
    /// Digest only: user input or credentials never appear in registry output.
    pub destination_hash: String,
    pub remote_id: Option<u64>,
    pub acknowledged: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<PublicationIdentity>,
}

/// Private, previously reviewed source and destination; never serialized in public views.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PublicationIdentity {
    pub host: String,
    pub owner: String,
    pub name: String,
    pub actor_login: String,
    pub actor_id: u64,
    pub owner_id: u64,
    pub target: crate::AccountTarget,
    pub account_basis: String,
    pub source_fingerprint: String,
}
impl PublicationIdentity {
    pub(crate) fn validate(&self) -> Result<()> {
        crate::github::validate_repository(&format!("{}/{}/{}", self.host, self.owner, self.name))
            .map_err(|_| anyhow::anyhow!("Invalid publication identity"))?;
        anyhow::ensure!(
            self.host == self.host.to_ascii_lowercase()
                && self.host.len() <= 253
                && self.owner.len() <= 256
                && self.name.len() <= 100
                && !self.actor_login.is_empty()
                && self.actor_login.len() <= 256
                && !self.actor_login.starts_with('-')
                && !matches!(self.actor_login.as_str(), "." | "..")
                && self
                    .actor_login
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.[]".contains(&b))
                && [&self.account_basis, &self.source_fingerprint]
                    .iter()
                    .all(|hash| hash.len() == 64
                        && hash
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
                && (1..=9_007_199_254_740_991).contains(&self.actor_id)
                && (1..=9_007_199_254_740_991).contains(&self.owner_id)
                && (!self.owner.eq_ignore_ascii_case(&self.actor_login)
                    || self.owner_id == self.actor_id),
            "Invalid publication identity"
        );
        match &self.target {
            crate::AccountTarget::Effective {} => {}
            crate::AccountTarget::Assigned { host, login } => anyhow::ensure!(
                host.eq_ignore_ascii_case(&self.host)
                    && login.eq_ignore_ascii_case(&self.actor_login),
                "Invalid publication account"
            ),
            crate::AccountTarget::None {} => anyhow::bail!("Invalid publication account"),
        }
        Ok(())
    }
    pub(crate) fn destination_hash(&self) -> Result<String> {
        use sha2::{Digest, Sha256};
        Ok(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(
                self.host.to_ascii_lowercase(),
                self.owner.to_ascii_lowercase(),
                self.name.to_ascii_lowercase()
            ))?)
        ))
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SavedRepositoryRecord {
    pub id: String,
    #[serde(with = "record_path")]
    pub path: PathBuf,
    pub account: StoredAccount,
}

mod record_path {
    use super::*;
    pub fn serialize<S: serde::Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
        match path.to_str() {
            Some(path) => serializer.serialize_str(path),
            None => crate::GitPath::Bytes {
                bytes: path.as_os_str().as_encoded_bytes().to_vec(),
            }
            .serialize(serializer),
        }
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<PathBuf, D::Error> {
        crate::directory_path(crate::GitPath::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}

fn legacy_id(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "legacy-{:x}",
        Sha256::digest(path.as_os_str().as_encoded_bytes())
    )
}

impl SavedRepositories {
    /// Public repository listings exclude private, authoritative recovery state.
    pub fn public_view(&self) -> Result<serde_json::Value> {
        let mut view = serde_json::to_value(self)?;
        view.as_object_mut()
            .context("Invalid saved repository view")?
            .remove("publication_intents");
        view.as_object_mut()
            .context("Invalid saved repository view")?
            .remove("clone_intents");
        Ok(view)
    }

    fn validate(&self) -> Result<()> {
        use std::collections::HashSet;
        let mut paths = HashSet::new();
        let mut clone_ids = HashSet::new();
        anyhow::ensure!(
            self.clone_intents.len() <= 2000,
            "Invalid clone recovery ledger"
        );
        for intent in &self.clone_intents {
            let parent = intent
                .stage
                .parent()
                .context("Invalid clone recovery stage")?;
            let name = parent
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix(".desktop-clone-"));
            anyhow::ensure!(
                uuid::Uuid::parse_str(&intent.id).is_ok_and(|id| id.to_string() == intent.id)
                    && matches!(intent.phase.as_str(), "downloading" | "checkout" | "ready")
                    && match (&*intent.phase, &intent.certificate) {
                        ("ready", Some(hash)) =>
                            hash.len() == 64
                                && hash
                                    .bytes()
                                    .all(|byte| byte.is_ascii_digit()
                                        || (b'a'..=b'f').contains(&byte)),
                        ("downloading" | "checkout", None) => true,
                        _ => false,
                    }
                    && intent
                        .url
                        .get(..8)
                        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"))
                    && !intent.url.contains(['@', '?', '#', '\n', '\r', '\0'])
                    && crate::RepositoryIdentity::from_url(&intent.url).is_ok()
                    && clone_ids.insert(&intent.id)
                    && intent.stage.is_absolute()
                    && intent.destination.is_absolute()
                    && intent
                        .stage
                        .file_name()
                        .is_some_and(|name| name == "repository")
                    && parent.parent() == intent.destination.parent()
                    && name
                        .is_some_and(|name| uuid::Uuid::parse_str(name)
                            .is_ok_and(|id| id.to_string() == name))
                    && matches!(
                        intent.account.assignment,
                        crate::AccountAssignment::Assigned { .. }
                    )
                    && intent.account.revision == 1
                    && intent.account.association.is_none()
                    && intent.account.source_remote.as_deref() == Some("origin"),
                "Invalid clone recovery ledger"
            );
            intent.account.validate()?;
            let identity = crate::RepositoryIdentity::from_url(&intent.url)?;
            let crate::AccountAssignment::Assigned { host, .. } = &intent.account.assignment else {
                unreachable!()
            };
            anyhow::ensure!(
                identity.host.eq_ignore_ascii_case(host),
                "Invalid clone recovery account host"
            );
        }
        for path in &self.repositories {
            anyhow::ensure!(
                path.is_absolute() && paths.insert(path),
                "Invalid saved repository paths"
            );
        }
        anyhow::ensure!(
            self.selected
                .as_ref()
                .is_none_or(|path| paths.contains(path)),
            "Selected repository is not saved"
        );
        let mut ids = HashSet::new();
        let mut records = HashSet::new();
        for record in &self.records {
            let valid_id = record.id.strip_prefix("legacy-").is_some_and(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            }) || uuid::Uuid::parse_str(&record.id)
                .is_ok_and(|id| id.to_string() == record.id);
            anyhow::ensure!(
                valid_id
                    && ids.insert(record.id.clone())
                    && records.insert(&record.path)
                    && paths.contains(&record.path),
                "Invalid saved repository records"
            );
            record.account.validate()?;
        }
        for path in &self.repositories {
            if !records.contains(path) {
                anyhow::ensure!(
                    ids.insert(legacy_id(path)),
                    "Invalid saved repository record identity"
                );
            }
        }
        anyhow::ensure!(
            self.publication_intents.len() <= 1000,
            "Invalid publication ledger"
        );
        let mut intent_ids = HashSet::new();
        let mut intent_repositories = HashSet::new();
        for intent in &self.publication_intents {
            let valid_hash = intent.repository_id.len() == 64
                && intent
                    .repository_id
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
            let valid_receipt =
                uuid::Uuid::parse_str(&intent.id).is_ok_and(|id| id.to_string() == intent.id);
            anyhow::ensure!(
                valid_hash
                    && valid_receipt
                    && (intent.common_id.is_empty()
                        || (intent.common_id.len() == 64
                            && intent
                                .common_id
                                .bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))))
                    && intent_ids.insert(&intent.id)
                    && intent_repositories.insert(&intent.repository_id)
                    && matches!(
                        intent.phase.as_str(),
                        "creation_unknown"
                            | "created"
                            | "origin_unknown"
                            | "origin_added"
                            | "assignment_unknown"
                            | "account_assigned"
                    )
                    && ((intent.phase != "creation_unknown") == intent.remote_id.is_some())
                    && (matches!(intent.phase.as_str(), "creation_unknown" | "created")
                        || intent.identity.is_some())
                    && intent
                        .remote_id
                        .is_none_or(|id| id > 0 && id <= 9_007_199_254_740_991)
                    && intent.destination_hash.len() == 64
                    && intent
                        .destination_hash
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    && (uuid::Uuid::parse_str(&intent.record_id)
                        .is_ok_and(|id| id.to_string() == intent.record_id)
                        || intent
                            .record_id
                            .strip_prefix("legacy-")
                            .is_some_and(|value| value.len() == 64
                                && value
                                    .bytes()
                                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))),
                "Invalid publication ledger"
            );
            if let Some(identity) = &intent.identity {
                identity.validate()?;
                anyhow::ensure!(
                    identity.destination_hash()? == intent.destination_hash,
                    "Invalid publication ledger"
                );
            }
        }
        Ok(())
    }
    fn record_for_path(&self, path: &Path) -> Option<SavedRepositoryRecord> {
        if !self.repositories.iter().any(|saved| saved == path) {
            return None;
        }
        Some(
            self.records
                .iter()
                .find(|record| record.path == path)
                .cloned()
                .unwrap_or_else(|| SavedRepositoryRecord {
                    id: legacy_id(path),
                    path: path.to_owned(),
                    account: StoredAccount::default(),
                }),
        )
    }
    pub(crate) fn record_by_id(&self, id: &str) -> Option<SavedRepositoryRecord> {
        self.records
            .iter()
            .find(|record| record.id == id)
            .cloned()
            .or_else(|| {
                self.repositories
                    .iter()
                    .filter(|path| !self.records.iter().any(|record| &record.path == *path))
                    .find(|path| legacy_id(path) == id)
                    .and_then(|path| self.record_for_path(path))
            })
    }
    pub(crate) fn persist_record(&mut self, record: SavedRepositoryRecord) {
        if let Some(saved) = self.records.iter_mut().find(|saved| saved.id == record.id) {
            *saved = record;
        } else {
            self.records.push(record);
        }
    }
}

impl Registry {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path: std::path::absolute(&path).unwrap_or(path),
            owned_handles: None,
        }
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn with_owned_handles(mut self, handles: Vec<std::sync::Arc<fs::File>>) -> Self {
        self.owned_handles = Some(std::sync::Arc::new(handles));
        self
    }

    /// Credential-free identity for this registry's configuration namespace.
    pub fn namespace_id(&self) -> String {
        use sha2::{Digest, Sha256};
        format!(
            "{:x}",
            Sha256::digest(self.path.as_os_str().as_encoded_bytes())
        )
    }

    pub fn default_path() -> Result<PathBuf> {
        let base = config_directory(|key| std::env::var_os(key))?;
        Ok(base.join("desktop-rs/repositories.json"))
    }

    pub fn read(&self) -> Result<SavedRepositories> {
        let mut data = match fs::read(&self.path) {
            Ok(bytes) => {
                let data: SavedRepositories = serde_json::from_slice(&bytes)
                    .context("read repository registry; existing data was not overwritten")?;
                if data.version != 1 {
                    bail!("unsupported repository registry version {}", data.version);
                }
                data.validate()?;
                data
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => SavedRepositories {
                version: 1,
                ..Default::default()
            },
            Err(error) => return Err(error.into()),
        };
        data.clone_intents = self.read_clone_intents()?;
        data.validate()?;
        Ok(data)
    }

    fn clone_intents_path(&self) -> PathBuf {
        let mut path = self.path.as_os_str().to_os_string();
        path.push(".clone-intents.json");
        path.into()
    }

    fn read_clone_intents(&self) -> Result<Vec<CloneIntent>> {
        let mut records = Vec::new();
        for (path, v2) in [
            (self.clone_intents_path(), false),
            (self.clone_intents_v2_path(), true),
        ] {
            if let Some(value) = read_private_sidecar(&path, 16 * 1024 * 1024)? {
                let entries = serde_json::from_slice::<Vec<CloneIntent>>(&value)
                    .context("Invalid clone recovery ledger")?;
                anyhow::ensure!(entries.len() <= 1000, "Invalid clone recovery ledger");
                anyhow::ensure!(
                    entries
                        .iter()
                        .all(|item| item.parent_identity.is_some() == v2),
                    "Invalid clone recovery ledger version"
                );
                records.extend(entries);
            }
        }
        Ok(records)
    }

    fn clone_intents_v2_path(&self) -> PathBuf {
        let mut path = self.path.as_os_str().to_os_string();
        path.push(".clone-intents-v2.json");
        path.into()
    }

    fn write_clone_intents(&self, previous: &[CloneIntent], next: &[CloneIntent]) -> Result<()> {
        let legacy = |items: &[CloneIntent]| {
            items
                .iter()
                .filter(|item| item.parent_identity.is_none())
                .cloned()
                .collect::<Vec<_>>()
        };
        anyhow::ensure!(
            serde_json::to_vec(&legacy(previous))? == serde_json::to_vec(&legacy(next))?,
            "Older clone recovery records require manual inspection"
        );
        let expected = serde_json::to_vec(
            &previous
                .iter()
                .filter(|item| item.parent_identity.is_some())
                .collect::<Vec<_>>(),
        )?;
        let next_v2 = next
            .iter()
            .filter(|item| item.parent_identity.is_some())
            .cloned()
            .collect::<Vec<_>>();
        anyhow::ensure!(
            serde_json::to_vec_pretty(&next_v2)?.len().saturating_add(1) <= 16 * 1024 * 1024,
            "Clone recovery ledger is full; inspect existing records"
        );
        let path = self.clone_intents_v2_path();
        crate::storage::update(
            &path,
            || match read_private_sidecar(&path, 16 * 1024 * 1024)? {
                Some(bytes) => Ok(serde_json::from_slice::<Vec<CloneIntent>>(&bytes)?),
                None => Ok(Vec::new()),
            },
            |current| {
                anyhow::ensure!(
                    serde_json::to_vec(current)? == expected,
                    "Clone recovery ledger changed"
                );
                *current = next_v2;
                Ok(())
            },
        )?;
        Ok(())
    }

    /// Call with a root returned by Repository::open. Select and deduplicate atomically.
    pub fn add(&self, root: PathBuf, select: bool) -> Result<SavedRepositories> {
        self.update(move |data| {
            let record = data
                .record_for_path(&root)
                .unwrap_or_else(|| SavedRepositoryRecord {
                    id: uuid::Uuid::new_v4().to_string(),
                    path: root.clone(),
                    account: StoredAccount::default(),
                });
            if !data.repositories.contains(&root) {
                data.repositories.push(root.clone());
            }
            data.persist_record(record);
            if select {
                data.selected = Some(root);
            }
            Ok(())
        })
    }

    /// Save a newly cloned root and its verified account together, without
    /// selecting it while checkout is still incomplete.
    fn add_assigned_clone(
        &self,
        root: PathBuf,
        account: StoredAccount,
    ) -> Result<SavedRepositoryRecord> {
        account.validate()?;
        let mut saved = None;
        self.update(|data| {
            anyhow::ensure!(
                data.record_for_path(&root).is_none(),
                "Clone destination is already saved; inspect it before retrying"
            );
            let record = SavedRepositoryRecord {
                id: uuid::Uuid::new_v4().to_string(),
                path: root.clone(),
                account: account.clone(),
            };
            data.repositories.push(root.clone());
            data.persist_record(record.clone());
            saved = Some(record);
            Ok(())
        })?;
        saved.context("Clone account could not be saved")
    }

    /// Persist recovery information before an irreversible directory publish.
    fn begin_clone_intent(&self, intent: CloneIntent) -> Result<()> {
        self.update(|data| {
            anyhow::ensure!(
                data.record_for_path(&intent.destination).is_none(),
                "Clone destination is already saved"
            );
            anyhow::ensure!(
                data.clone_intents
                    .iter()
                    .filter(|saved| saved.parent_identity.is_some())
                    .count()
                    < 1000,
                "Clone recovery ledger is full; inspect existing records"
            );
            anyhow::ensure!(
                !data
                    .clone_intents
                    .iter()
                    .any(|saved| saved.destination == intent.destination),
                "Another clone already needs recovery for this destination"
            );
            data.clone_intents.push(intent.clone());
            Ok(())
        })?;
        Ok(())
    }

    /// Complete a verified publication without changing an unrelated selection.
    fn finish_clone_intent(
        &self,
        intent: &CloneIntent,
        select: bool,
    ) -> Result<SavedRepositoryRecord> {
        // Hash outside the global registry lock; the short locked section
        // checks both the ledger and the destination's directory identity.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let (parent, certificate_path) = pinned_clone_destination(intent)?;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        verify_clone_certificate(&certificate_path, intent)?;
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        verify_clone_identity(&intent.destination, intent.device, intent.inode)?;
        let mut completed = None;
        self.update(|data| {
            anyhow::ensure!(
                data.clone_intents
                    .iter()
                    .filter(|saved| saved.destination == intent.destination)
                    .count()
                    == 1,
                "Conflicting clone recovery records require manual inspection"
            );
            let saved = data
                .clone_intents
                .iter()
                .find(|saved| saved.id == intent.id)
                .context("Clone recovery record changed")?;
            anyhow::ensure!(
                saved.phase == "ready"
                    && saved.certificate == intent.certificate
                    && saved.url == intent.url
                    && saved.stage == intent.stage
                    && saved.destination == intent.destination
                    && saved.device == intent.device
                    && saved.inode == intent.inode
                    && saved.parent_identity == intent.parent_identity
                    && serde_json::to_vec(&saved.account)? == serde_json::to_vec(&intent.account)?,
                "Clone recovery record changed"
            );
            #[cfg(any(target_os = "linux", target_os = "android"))]
            {
                let _ = pinned_clone_destination(intent)?;
            }
            #[cfg(not(any(target_os = "linux", target_os = "android")))]
            verify_clone_identity(&intent.destination, intent.device, intent.inode)?;
            let record = if let Some(existing) = data.record_for_path(&intent.destination) {
                anyhow::ensure!(
                    existing.id == intent.id
                        && serde_json::to_vec(&existing.account)?
                            == serde_json::to_vec(&saved.account)?,
                    "Clone destination was saved separately; inspect it before retrying"
                );
                existing
            } else {
                anyhow::ensure!(
                    data.record_by_id(&intent.id).is_none(),
                    "Clone identity was saved elsewhere; inspect it before retrying"
                );
                let record = SavedRepositoryRecord {
                    id: intent.id.clone(),
                    path: intent.destination.clone(),
                    account: saved.account.clone(),
                };
                data.repositories.push(record.path.clone());
                data.persist_record(record.clone());
                record
            };
            if select {
                data.selected = Some(record.path.clone());
            }
            completed = Some(record);
            Ok(())
        })?;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        drop(parent);
        let completed = completed.context("Clone publication could not be saved")?;
        // The main registry must be durable before removing the separate
        // journal. If interrupted here, the stable record ID makes replay
        // idempotent and never adopts another repository's account.
        self.update(|data| {
            anyhow::ensure!(
                data.clone_intents
                    .iter()
                    .filter(|saved| saved.destination == intent.destination)
                    .count()
                    == 1,
                "Conflicting clone recovery records require manual inspection"
            );
            let saved = data
                .clone_intents
                .iter()
                .find(|saved| saved.id == intent.id)
                .context("Clone recovery record changed")?;
            let record = data
                .record_by_id(&intent.id)
                .context("Clone publication record changed")?;
            anyhow::ensure!(
                saved.phase == "ready"
                    && saved.certificate == intent.certificate
                    && saved.url == intent.url
                    && saved.stage == intent.stage
                    && saved.destination == intent.destination
                    && saved.device == intent.device
                    && saved.inode == intent.inode
                    && saved.parent_identity == intent.parent_identity
                    && serde_json::to_vec(&saved.account)? == serde_json::to_vec(&intent.account)?
                    && record.path == intent.destination
                    && serde_json::to_vec(&record.account)? == serde_json::to_vec(&intent.account)?,
                "Clone publication record changed"
            );
            data.clone_intents.retain(|saved| saved.id != intent.id);
            Ok(())
        })?;
        Ok(completed)
    }

    fn advance_clone_intent(
        &self,
        intent: &CloneIntent,
        from: &str,
        to: &str,
        certificate: Option<String>,
    ) -> Result<()> {
        anyhow::ensure!(
            matches!(
                (from, to),
                ("downloading", "checkout") | ("checkout", "ready")
            ),
            "Invalid clone recovery transition"
        );
        anyhow::ensure!(
            (to == "ready") == certificate.is_some(),
            "Invalid clone recovery certificate"
        );
        self.update(|data| {
            anyhow::ensure!(
                data.clone_intents
                    .iter()
                    .filter(|saved| saved.destination == intent.destination)
                    .count()
                    == 1,
                "Conflicting clone recovery records require manual inspection"
            );
            let saved = data
                .clone_intents
                .iter_mut()
                .find(|saved| saved.id == intent.id)
                .context("Clone recovery record changed")?;
            anyhow::ensure!(
                saved.phase == from
                    && saved.url == intent.url
                    && saved.stage == intent.stage
                    && saved.destination == intent.destination
                    && saved.device == intent.device
                    && saved.inode == intent.inode
                    && saved.parent_identity == intent.parent_identity
                    && serde_json::to_vec(&saved.account)? == serde_json::to_vec(&intent.account)?,
                "Clone recovery record changed"
            );
            saved.phase = to.into();
            saved.certificate = certificate.clone();
            Ok(())
        })?;
        Ok(())
    }

    fn ensure_uncontested_clone(&self, intent: &CloneIntent) -> Result<()> {
        let state = self.read()?;
        anyhow::ensure!(
            state
                .clone_intents
                .iter()
                .filter(|saved| saved.destination == intent.destination)
                .count()
                == 1,
            "Conflicting clone recovery records require manual inspection"
        );
        Ok(())
    }

    /// Recovery records contain no credential material. They are returned only
    /// on an explicit recovery request, never in the regular repository list.
    pub fn clone_recovery_records(&self) -> Result<serde_json::Value> {
        let state = self.read()?;
        let mut counts = std::collections::HashMap::new();
        for intent in &state.clone_intents {
            *counts.entry(&intent.destination).or_insert(0_usize) += 1;
        }
        let records = state
            .clone_intents
            .iter()
            .map(|intent| {
                let conflicted = counts.get(&intent.destination) != Some(&1);
                let recoverable =
                    !conflicted && intent.parent_identity.is_some() && intent.phase == "ready";
                let mut value = serde_json::to_value(intent)?;
                let object = value
                    .as_object_mut()
                    .context("Invalid clone recovery record")?;
                // Recovery UIs need only the paths and phase. URL, account, file
                // identities and certificates remain private to the journal.
                object.retain(|key, _| {
                    matches!(key.as_str(), "id" | "phase" | "stage" | "destination")
                });
                object.insert("recoverable".into(), serde_json::Value::Bool(recoverable));
                object.insert("conflicted".into(), serde_json::Value::Bool(conflicted));
                Ok(value)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(serde_json::Value::Array(records))
    }

    pub fn select(&self, root: PathBuf) -> Result<SavedRepositories> {
        self.update(move |data| {
            if !data.repositories.contains(&root) {
                bail!("repository is not saved; add it first");
            }
            let record = data
                .record_for_path(&root)
                .context("Repository is not saved")?;
            data.persist_record(record);
            data.selected = Some(root);
            Ok(())
        })
    }

    /// Clear only the current selection; saved repositories and accounts remain.
    pub fn clear_selection(&self) -> Result<SavedRepositories> {
        self.update(|data| {
            data.selected = None;
            Ok(())
        })
    }

    /// Select a previously bound record without recreating a forgotten path.
    pub fn select_record(&self, id: &str, expected_path: &Path) -> Result<SavedRepositoryRecord> {
        let mut selected = None;
        self.update(|data| {
            let record = data
                .record_by_id(id)
                .context("Saved repository changed; select it again")?;
            anyhow::ensure!(
                record.path == expected_path,
                "Saved repository working tree changed; select it again"
            );
            data.persist_record(record.clone());
            data.selected = Some(record.path.clone());
            selected = Some(record);
            Ok(())
        })?;
        selected.context("Saved repository was not selected")
    }

    /// Forget metadata only. Repository contents are never removed.
    pub fn forget(&self, root: &Path) -> Result<SavedRepositories> {
        self.update(|data| {
            data.repositories.retain(|path| path != root);
            data.records.retain(|record| record.path != root);
            if data.selected.as_deref() == Some(root) {
                data.selected = None;
            }
            Ok(())
        })
    }

    /// Exact saved-root lookup; legacy entries are synthesized without writing.
    pub fn record_for_path(&self, path: &Path) -> Result<Option<SavedRepositoryRecord>> {
        Ok(self.read()?.record_for_path(path))
    }

    pub fn record_by_id(&self, id: &str) -> Result<Option<SavedRepositoryRecord>> {
        Ok(self.read()?.record_by_id(id))
    }

    /// Publish an account change only if the saved record and revision still exist.
    pub fn compare_and_replace_account(
        &self,
        id: &str,
        expected_revision: u64,
        account: StoredAccount,
    ) -> Result<SavedRepositoryRecord> {
        anyhow::ensure!(
            account.revision
                == expected_revision
                    .checked_add(1)
                    .context("Account revision overflow")?,
            "Invalid replacement account revision"
        );
        let mut updated = None;
        self.update(|data| {
            let mut record = data
                .record_by_id(id)
                .context("Saved repository changed; select it again")?;
            anyhow::ensure!(
                record.account.revision == expected_revision,
                "Repository account changed; load it again"
            );
            record.account = account;
            data.persist_record(record.clone());
            updated = Some(record);
            Ok(())
        })?;
        updated.context("Saved repository account was not updated")
    }

    /// Move a reviewed saved worktree record without stealing an unrelated
    /// selection or replacing a destination's separately saved identity.
    pub(crate) fn relocate_worktree_record(
        &self,
        source: &SavedRepositoryRecord,
        target: PathBuf,
        follow_current: bool,
    ) -> Result<Option<SavedRepositoryRecord>> {
        let mut moved = None;
        self.update(|data| {
            let Some(mut record) = data.record_by_id(&source.id) else {
                return Ok(());
            };
            anyhow::ensure!(
                record.path == source.path && record.account.revision == source.account.revision,
                "Saved worktree account changed; reload before moving it"
            );
            if target == source.path {
                moved = Some(record);
                return Ok(());
            }
            anyhow::ensure!(
                data.record_for_path(&target).is_none(),
                "The destination already has a saved repository identity"
            );
            for path in &mut data.repositories {
                if *path == source.path {
                    *path = target.clone();
                }
            }
            if follow_current && data.selected.as_deref() == Some(source.path.as_path()) {
                data.selected = Some(target.clone());
            } else if data.selected.as_deref() == Some(source.path.as_path()) {
                data.selected = None;
            }
            record.path = target.clone();
            data.persist_record(record.clone());
            moved = Some(record);
            Ok(())
        })?;
        Ok(moved)
    }

    /// Keep a saved record stable while performing bounded local recovery I/O.
    pub(crate) fn with_record_lock<T>(
        &self,
        id: &str,
        path: &Path,
        work: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let _lock = crate::storage::lock(&self.path)?;
        let record = self
            .read()?
            .record_by_id(id)
            .context("Saved repository changed; select it again")?;
        anyhow::ensure!(
            record.path == path,
            "Saved repository working tree changed; select it again"
        );
        work()
    }

    /// Adopt an existing target record, otherwise relocate the source record intact.
    /// The caller must first verify native Git worktree membership.
    pub fn switch_worktree(
        &self,
        source_id: Option<&str>,
        target: PathBuf,
    ) -> Result<SavedRepositoryRecord> {
        let source = source_id
            .map(|id| {
                self.record_by_id(id)?
                    .context("Saved repository changed; select it again")
            })
            .transpose()?;
        self.switch_worktree_record(source.as_ref(), target)
    }

    pub(crate) fn switch_worktree_record(
        &self,
        source: Option<&SavedRepositoryRecord>,
        target: PathBuf,
    ) -> Result<SavedRepositoryRecord> {
        let mut selected = None;
        self.update(|data| {
            let source = source
                .map(|expected| {
                    let record = data
                        .record_by_id(&expected.id)
                        .context("Saved repository changed; select it again")?;
                    anyhow::ensure!(
                        record.path == expected.path,
                        "Saved repository working tree changed; select it again"
                    );
                    Ok::<_, anyhow::Error>(record)
                })
                .transpose()?;
            let record = if let Some(record) = data.record_for_path(&target) {
                record
            } else if let Some(mut source) = source {
                let previous = source.path.clone();
                source.path = target.clone();
                for path in &mut data.repositories {
                    if *path == previous {
                        *path = target.clone();
                    }
                }
                source
            } else {
                data.repositories.push(target.clone());
                SavedRepositoryRecord {
                    id: uuid::Uuid::new_v4().to_string(),
                    path: target.clone(),
                    account: StoredAccount::default(),
                }
            };
            data.persist_record(record.clone());
            data.selected = Some(target);
            selected = Some(record);
            Ok(())
        })?;
        selected.context("Working tree was not selected")
    }

    fn update(
        &self,
        action: impl FnOnce(&mut SavedRepositories) -> Result<()>,
    ) -> Result<SavedRepositories> {
        crate::storage::update(
            &self.path,
            || self.read(),
            |data| {
                let prior_intents = data.clone_intents.clone();
                action(data)?;
                data.validate()?;
                if serde_json::to_vec(&data.clone_intents)? != serde_json::to_vec(&prior_intents)? {
                    self.write_clone_intents(&prior_intents, &data.clone_intents)?;
                }
                Ok(())
            },
        )
    }
    /// Publication intents live in the private registry, not in an untrusted Git directory.
    /// Use the same lock filename as ordinary updates, but open every file without
    /// following symlinks and replace the registry relative to the checked directory.
    pub(crate) fn publication_store<T>(
        &self,
        repository: &Path,
        git_directory: &Path,
        common_directory: &Path,
        write: bool,
        work: impl FnOnce(&mut SavedRepositories) -> Result<T>,
    ) -> Result<T> {
        use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
        use cap_std::{
            ambient_authority,
            fs::{Dir, OpenOptions},
        };
        use std::io::{Read, Write};
        let parent = self.path.parent().context("Invalid publication registry")?;
        let canonical = parent
            .canonicalize()
            .context("Invalid publication registry")?;
        let worktree = repository.canonicalize().context("Invalid repository")?;
        let git = git_directory
            .canonicalize()
            .context("Invalid Git directory")?;
        let common = common_directory
            .canonicalize()
            .context("Invalid Git common directory")?;
        anyhow::ensure!(
            canonical == parent
                && !canonical.starts_with(&worktree)
                && !canonical.starts_with(&git)
                && !canonical.starts_with(&common),
            "Publication registry must be outside the repository"
        );
        // Resolve every component through an already-open directory, preventing
        // a symlink replacement between validation and opening the registry.
        #[cfg(unix)]
        let dir = {
            use std::path::Component;
            let mut directory = Dir::open_ambient_dir("/", ambient_authority())?;
            for component in parent.components() {
                match component {
                    Component::RootDir => {}
                    Component::Normal(name) => {
                        directory = directory
                            .open_dir_nofollow(name)
                            .map_err(|_| anyhow::anyhow!("Invalid publication registry"))?
                    }
                    _ => anyhow::bail!("Invalid publication registry"),
                }
            }
            use std::os::unix::fs::MetadataExt;
            let metadata = directory.open(".")?.into_std().metadata()?;
            anyhow::ensure!(
                metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o022 == 0,
                "Publication registry directory must be private"
            );
            directory
        };
        #[cfg(not(unix))]
        let dir: Dir = { anyhow::bail!("Publication registry is unavailable on this platform") };
        let file_name = self
            .path
            .file_name()
            .context("Invalid publication registry")?;
        let lock_path = self.path.with_extension("lock");
        let lock_name = lock_path
            .file_name()
            .context("Invalid publication registry")?;
        let mut lock_options = OpenOptions::new();
        lock_options
            .read(true)
            .write(true)
            .follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            lock_options.custom_flags(libc::O_NONBLOCK);
        }
        let lock = dir.open_with(lock_name, &lock_options)?.into_std();
        anyhow::ensure!(
            lock.metadata()?.is_file(),
            "Invalid publication registry lock"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            anyhow::ensure!(
                lock.metadata()?.nlink() == 1
                    && lock.metadata()?.uid() == unsafe { libc::geteuid() }
                    && lock.metadata()?.mode() & 0o077 == 0,
                "Invalid publication registry lock"
            );
        }
        loop {
            crate::cancellation::Cancellation::current().check()?;
            match lock.try_lock() {
                Ok(()) => break,
                Err(std::fs::TryLockError::WouldBlock) => {
                    std::thread::sleep(std::time::Duration::from_millis(10))
                }
                Err(std::fs::TryLockError::Error(_)) => {
                    anyhow::bail!("Cannot lock publication registry")
                }
            }
        }
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let mut file = dir.open_with(file_name, &options)?.into_std();
        let metadata = file.metadata()?;
        anyhow::ensure!(metadata.is_file(), "Invalid publication registry");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            anyhow::ensure!(
                metadata.nlink() == 1
                    && metadata.uid() == unsafe { libc::geteuid() }
                    && metadata.mode() & 0o077 == 0,
                "Publication registry must be private"
            );
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        anyhow::ensure!(
            bytes.len() <= 16 * 1024 * 1024,
            "Publication registry is too large"
        );
        let mut data: SavedRepositories = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Invalid publication registry"))?;
        anyhow::ensure!(data.version == 1, "Invalid publication registry");
        data.validate()?;
        let result = work(&mut data)?;
        if write {
            data.validate()?;
            let encoded = serde_json::to_vec_pretty(&data)?;
            anyhow::ensure!(
                encoded.len() <= 16 * 1024 * 1024,
                "Publication registry is too large"
            );
            let tmp = format!(".publication-{}.tmp", uuid::Uuid::new_v4());
            let mut options = OpenOptions::new();
            options
                .write(true)
                .create_new(true)
                .follow(FollowSymlinks::No);
            #[cfg(unix)]
            {
                use cap_std::fs::OpenOptionsExt;
                options.mode(0o600).custom_flags(libc::O_NONBLOCK);
            }
            let saved = (|| -> Result<()> {
                let mut output = dir.open_with(&tmp, &options)?;
                output.write_all(&encoded)?;
                output.write_all(b"\n")?;
                output.sync_all()?;
                dir.rename(&tmp, &dir, file_name)?;
                dir.open(".")?.into_std().sync_all()?;
                Ok(())
            })();
            if saved.is_err() {
                let _ = dir.remove_file(&tmp);
            }
            saved.map_err(|_| anyhow::anyhow!("Cannot persist publication registry"))?;
        }
        Ok(result)
    }

    pub(crate) fn publication_record_matches(
        data: &SavedRepositories,
        id: &str,
        path: &Path,
    ) -> bool {
        data.record_by_id(id)
            .is_some_and(|record| record.path == path)
    }

    pub fn preferences(&self) -> crate::PreferencesStore {
        crate::PreferencesStore::new(self.path.with_extension("preferences.json"))
    }
}

#[cfg(unix)]
fn verify_clone_identity(path: &Path, device: u64, inode: u64) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path).context("Clone destination cannot be inspected")?;
    anyhow::ensure!(
        metadata.file_type().is_dir() && metadata.dev() == device && metadata.ino() == inode,
        "Clone destination changed; inspect it before recovery"
    );
    Ok(())
}

fn clone_certificate(path: &Path) -> Result<String> {
    Ok(crate::renderer_worktree::removal_tree_certificate(path)?
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn verify_clone_certificate(path: &Path, intent: &CloneIntent) -> Result<()> {
    let expected = intent
        .certificate
        .as_deref()
        .context("Clone recovery certificate is missing")?;
    anyhow::ensure!(
        clone_certificate(path)? == expected,
        "Clone contents changed; inspect the staging directory before recovery"
    );
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) fn pinned_clone_directory(handle: &fs::File, proc_root: &Path) -> Result<PathBuf> {
    use std::os::{fd::AsRawFd, unix::fs::MetadataExt};
    let expected = handle.metadata()?;
    anyhow::ensure!(expected.is_dir(), "Clone staging path is not a directory");
    let path = proc_root
        .join(std::process::id().to_string())
        .join("fd")
        .join(handle.as_raw_fd().to_string());
    let actual = fs::metadata(&path)
        .context("Selected-account clone requires an accessible process descriptor filesystem")?;
    anyhow::ensure!(
        actual.is_dir() && actual.dev() == expected.dev() && actual.ino() == expected.ino(),
        "Cannot safely pin clone staging directory"
    );
    Ok(path)
}
/// Walk an already-canonical parent without ever following a substituted link.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) fn open_clone_parent(path: &Path) -> Result<fs::File> {
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    };
    use std::path::Component;
    anyhow::ensure!(path.is_absolute(), "Clone parent must be absolute");
    let mut directory = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open("/")?;
    let mut expected = PathBuf::from("/");
    for component in path.components() {
        let Component::Normal(name) = component else {
            anyhow::ensure!(
                matches!(component, Component::RootDir),
                "Invalid clone parent component"
            );
            continue;
        };
        let name =
            std::ffi::CString::new(name.as_bytes()).context("Invalid clone parent component")?;
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error()).context("Cannot safely open clone parent");
        }
        directory = unsafe { fs::File::from_raw_fd(fd) };
        expected.push(std::ffi::OsStr::from_bytes(name.as_bytes()));
        let pinned = pinned_clone_directory(&directory, Path::new("/proc"))?;
        anyhow::ensure!(
            fs::canonicalize(&pinned)? == expected,
            "Clone parent changed while opening directories"
        );
    }
    Ok(directory)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn canonical_clone_destination(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let name = absolute
        .file_name()
        .context("destination needs a directory name")?;
    let parent = absolute
        .parent()
        .context("destination needs a parent directory")?
        .canonicalize()
        .context("Selected-account clone requires an existing destination parent")?;
    Ok(parent.join(name))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn ensure_clone_destination_outside_sensitive_home(destination: &Path, home: &Path) -> Result<()> {
    anyhow::ensure!(
        destination != home
            && ![".ssh", ".gnupg", ".config", ".gitconfig"]
                .iter()
                .any(|name| {
                    let path = home.join(name);
                    destination.starts_with(&path)
                        || fs::canonicalize(&path)
                            .is_ok_and(|resolved| destination.starts_with(resolved))
                }),
        "Clone destination targets a sensitive system location"
    );
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn revalidate_clone_parent(handle: &fs::File, expected: &Path, home: &Path) -> Result<()> {
    let pinned = pinned_clone_directory(handle, Path::new("/proc"))?;
    let actual = fs::canonicalize(pinned).context("Clone parent is unavailable")?;
    anyhow::ensure!(
        actual == expected,
        "Clone parent changed during account verification"
    );
    ensure_clone_destination_outside_sensitive_home(&actual.join(".desktop-clone-check"), home)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn pinned_clone_destination(intent: &CloneIntent) -> Result<(fs::File, PathBuf)> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let parent_path = intent
        .destination
        .parent()
        .context("Invalid clone destination")?;
    let parent = open_clone_parent(parent_path)?;
    let metadata = parent.metadata()?;
    anyhow::ensure!(
        Some((metadata.dev(), metadata.ino())) == intent.parent_identity,
        "Clone destination parent changed; inspect it before recovery"
    );
    let path = pinned_clone_directory(&parent, Path::new("/proc"))?.join(
        intent
            .destination
            .file_name()
            .context("Invalid clone destination")?,
    );
    let child = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)?;
    let metadata = child.metadata()?;
    anyhow::ensure!(
        (metadata.dev(), metadata.ino()) == (intent.device, intent.inode),
        "Published clone directory changed"
    );
    Ok((parent, path))
}

/// Publish or resume publication of an owned staging directory without ever
/// replacing an existing destination. The source and target parents remain
/// pinned by directory handles across the rename and identity check.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn publish_clone_intent(registry: &Registry, intent: &CloneIntent) -> Result<()> {
    publish_clone_intent_before_rename(registry, intent, || Ok(()), || Ok(()))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn publish_clone_intent_before_rename(
    registry: &Registry,
    intent: &CloneIntent,
    before_lock: impl FnOnce() -> Result<()>,
    after_recheck: impl FnOnce() -> Result<()>,
) -> Result<()> {
    use cap_std::fs::Dir;
    use std::os::unix::fs::MetadataExt;
    use std::{
        ffi::{CString, OsStr},
        os::{fd::AsRawFd, unix::ffi::OsStrExt},
    };
    #[allow(clippy::unnecessary_cast)] // libc field widths differ on 32-bit targets.
    fn identity(directory: &Dir, name: &OsStr) -> Result<Option<(u64, u64)>> {
        let name = CString::new(name.as_bytes()).context("Invalid clone path")?;
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // The dirfd and C string remain valid throughout this non-following stat.
        let result = unsafe {
            libc::fstatat(
                directory.as_raw_fd(),
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }
            return Err(error).context("Cannot inspect clone publication");
        }
        // fstatat initialized the structure on success.
        let stat = unsafe { stat.assume_init() };
        anyhow::ensure!(
            stat.st_mode & libc::S_IFMT == libc::S_IFDIR,
            "Clone publication path is not a directory"
        );
        Ok(Some((stat.st_dev as u64, stat.st_ino as u64)))
    }
    anyhow::ensure!(
        intent.phase == "ready",
        "Clone is not ready for publication; inspect the staging directory"
    );
    let source_parent = intent
        .stage
        .parent()
        .context("Invalid clone staging path")?;
    let target_parent = intent
        .destination
        .parent()
        .context("Invalid clone destination")?;
    let source_name = intent
        .stage
        .file_name()
        .context("Invalid clone staging name")?;
    let target_name = intent
        .destination
        .file_name()
        .context("Invalid clone destination name")?;
    let expected_parent = intent
        .parent_identity
        .context("Older clone recovery requires manual inspection")?;
    let target_handle =
        open_clone_parent(target_parent).context("Clone destination directory is unavailable")?;
    let metadata = target_handle.metadata()?;
    anyhow::ensure!(
        (metadata.dev(), metadata.ino()) == expected_parent,
        "Clone destination parent changed; inspect it before recovery"
    );
    let source_handle =
        open_clone_parent(source_parent).context("Clone staging directory is unavailable")?;
    let source_dir = Dir::from_std_file(source_handle);
    let target_dir = Dir::from_std_file(target_handle);
    let expected = (intent.device, intent.inode);
    if let Some(actual) = identity(&source_dir, source_name)? {
        anyhow::ensure!(
            actual == expected,
            "Clone staging directory changed; inspect it before recovery"
        );
        let pinned_source = PathBuf::from(format!(
            "/proc/{}/fd/{}",
            std::process::id(),
            source_dir.as_raw_fd()
        ))
        .join(source_name);
        verify_clone_certificate(&pinned_source, intent)?;
        anyhow::ensure!(
            identity(&source_dir, source_name)? == Some(expected),
            "Clone staging directory changed during review"
        );
        before_lock()?;
        // Legacy writers use this same lock. Keep it through the short
        // identity check and rename, never through the tree scan above.
        let lock = crate::storage::lock(&registry.path)?;
        let state = registry.read()?;
        anyhow::ensure!(
            state
                .clone_intents
                .iter()
                .filter(|saved| saved.destination == intent.destination)
                .count()
                == 1,
            "Conflicting clone recovery records require manual inspection"
        );
        let saved = state
            .clone_intents
            .iter()
            .find(|saved| saved.id == intent.id)
            .context("Clone recovery record changed")?;
        anyhow::ensure!(
            saved.phase == "ready"
                && saved.certificate == intent.certificate
                && saved.url == intent.url
                && saved.stage == intent.stage
                && saved.destination == intent.destination
                && saved.device == intent.device
                && saved.inode == intent.inode
                && saved.parent_identity == intent.parent_identity
                && serde_json::to_vec(&saved.account)? == serde_json::to_vec(&intent.account)?,
            "Clone recovery record changed"
        );
        anyhow::ensure!(
            identity(&source_dir, source_name)? == Some(expected),
            "Clone staging directory changed during review"
        );
        after_recheck()?;
        crate::cancellation::Cancellation::current().check()?;
        let source = CString::new(source_name.as_bytes()).context("Invalid clone staging name")?;
        let target =
            CString::new(target_name.as_bytes()).context("Invalid clone destination name")?;
        // Both names are single path components and both parent FDs remain live.
        let result = unsafe {
            libc::renameat2(
                source_dir.as_raw_fd(),
                source.as_ptr(),
                target_dir.as_raw_fd(),
                target.as_ptr(),
                libc::RENAME_NOREPLACE as _,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error())
                .context("Clone destination cannot be published without replacing existing data");
        }
        drop(lock);
        source_dir.open(".")?.into_std().sync_all()?;
        target_dir.open(".")?.into_std().sync_all()?;
    }
    anyhow::ensure!(
        identity(&target_dir, target_name)? == Some(expected),
        "Published clone differs from the reviewed staging directory"
    );
    verify_clone_identity(&intent.destination, intent.device, intent.inode)?;
    let pinned_target = PathBuf::from(format!(
        "/proc/{}/fd/{}",
        std::process::id(),
        target_dir.as_raw_fd()
    ))
    .join(target_name);
    verify_clone_certificate(&pinned_target, intent)
}

#[cfg(not(unix))]
fn verify_clone_identity(_path: &Path, _device: u64, _inode: u64) -> Result<()> {
    bail!("Clone recovery is unavailable on this platform")
}

/// Initialize a new working tree without reinitializing an existing repository.
pub async fn initialize(path: &Path) -> Result<Repository> {
    let destination = prepare_destination(path)?;
    let parent = destination
        .parent()
        .context("repository needs a parent directory")?;
    let policy = process::run(parent, &args(&["config", "--get", "init.defaultbranch"])).await?;
    if !matches!(policy.code, 0 | 1) {
        bail!("{}", policy.text().stderr);
    }
    let mut command = args(&["init"]);
    if policy.code == 1 {
        command.push("--initial-branch=main".into());
    }
    command.push("--".into());
    command.push(destination.as_os_str().to_owned());
    let output = process::run(parent, &command).await?;
    if output.code != 0 {
        bail!("{}", output.text().stderr);
    }
    Repository::open(&destination).await
}

/// Initialize the directory chosen in Desktop without selecting or registering it.
pub async fn initialize_desktop_repository(path: &Path, branch: &str) -> Result<()> {
    anyhow::ensure!(path.is_absolute(), "repository path must be absolute");
    anyhow::ensure!(
        path.file_name().is_some(),
        "repository path must name a directory"
    );
    anyhow::ensure!(
        !branch.is_empty()
            && branch.len() <= 4096
            && !branch.starts_with('-')
            && !branch.contains("@{"),
        "invalid default branch"
    );
    let check = process::initialization_command(
        &std::env::current_dir()?,
        &args(&["check-ref-format", "--branch", branch]),
    )
    .await?;
    anyhow::ensure!(check.code == 0, "invalid default branch");
    let requested = path.to_owned();
    let directory = crate::cancellation::blocking(move || {
        fs::create_dir_all(&requested)?;
        Ok(requested.canonicalize()?)
    })
    .await?;
    anyhow::ensure!(
        directory.parent().is_some(),
        "filesystem root cannot be initialized here"
    );
    let bare =
        process::initialization_command(&directory, &args(&["rev-parse", "--is-bare-repository"]))
            .await?;
    anyhow::ensure!(
        matches!(bare.code, 0 | 128),
        "Unable to inspect repository destination"
    );
    anyhow::ensure!(
        bare.code != 0
            || !String::from_utf8_lossy(&bare.stdout)
                .trim()
                .eq_ignore_ascii_case("true"),
        "Destination is already a bare repository"
    );
    let git_directory = directory.join(".git");
    crate::cancellation::blocking(move || {
        fs::create_dir(&git_directory)
            .context("Destination already has a Git directory or cannot be initialized")?;
        Ok(())
    })
    .await?;
    let setting = format!("init.defaultBranch={branch}");
    let output =
        process::initialization_command(&directory, &args(&["-c", &setting, "init"])).await?;
    anyhow::ensure!(
        output.code == 0,
        "Repository initialization failed; inspect the destination before retrying"
    );
    Ok(())
}

/// Clone using system Git and its credential configuration. No secrets enter the registry.
pub async fn clone_repository(url: &str, path: &Path) -> Result<Repository> {
    clone_repository_with_options(url, path, CloneOptions::default()).await
}

/// Clone options accepted by the native clone endpoint. Defaults preserve the
/// original endpoint's behavior for existing callers.
#[derive(Default)]
pub struct CloneOptions {
    pub branch: Option<String>,
    pub default_branch: Option<String>,
    pub recursive: bool,
    pub report_progress: bool,
}

/// An explicitly selected stored account; its numeric actor is verified before
/// any clone transport receives credentials.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloneAccount {
    pub host: String,
    pub login: String,
    pub id: u64,
}

/// Clone privately with a selected account. A write-ahead recovery record
/// precedes transport and publication never replaces data.
#[cfg(any(target_os = "linux", target_os = "android"))]
async fn clone_repository_selected_unavailable_implementation(
    registry: Registry,
    url: &str,
    path: &Path,
    options: CloneOptions,
    account: CloneAccount,
) -> Result<Repository> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
    anyhow::ensure!(
        !options.recursive,
        "Selected-account recursive clones are not supported yet"
    );
    for value in [&options.branch, &options.default_branch]
        .into_iter()
        .flatten()
    {
        anyhow::ensure!(
            !value.is_empty()
                && value.len() <= 4096
                && !value.starts_with('-')
                && !value.chars().any(char::is_control),
            "Invalid clone branch"
        );
    }
    let destination = canonical_clone_destination(path)?;
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .context("Cannot inspect the user's home directory")?;
    let home = fs::canonicalize(home).context("Cannot inspect the user's home directory")?;
    ensure_clone_destination_outside_sensitive_home(&destination, &home)?;
    match fs::symlink_metadata(&destination) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => bail!("Selected-account clone requires a destination that does not yet exist"),
        Err(error) => return Err(error).context("Cannot inspect clone destination"),
    }
    let parent = destination
        .parent()
        .context("repository needs a parent directory")?;
    let parent_handle = std::sync::Arc::new(
        open_clone_parent(parent)
            .context("Selected-account clone requires an existing destination parent")?,
    );
    let parent_metadata = parent_handle.metadata()?;
    let pinned_parent = pinned_clone_directory(&parent_handle, Path::new("/proc"))?;
    let host = account.host.to_ascii_lowercase();
    let chosen = crate::github_api::ChosenAccount {
        host: &host,
        login: &account.login,
        id: Some(account.id),
    };
    crate::git_credentials::clone::verify_clone_account(&pinned_parent, url, chosen).await?;
    let rewritten = process::run(
        &pinned_parent,
        &args(&["ls-remote", "--get-url", "--", url]),
    )
    .await?;
    anyhow::ensure!(
        rewritten.code == 0 && rewritten.stdout.strip_suffix(b"\n") == Some(url.as_bytes()),
        "Selected-account clone does not support URL rewrites"
    );
    revalidate_clone_parent(&parent_handle, parent, &home)?;
    let clone_id = uuid::Uuid::new_v4().to_string();
    let container = parent.join(format!(".desktop-clone-{clone_id}"));
    let anchored_container = pinned_parent.join(
        container
            .file_name()
            .context("Invalid clone staging name")?,
    );
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&anchored_container)
        .context("Cannot create private clone staging directory")?;
    macro_rules! staging_try {
        ($operation:expr) => {
            $operation.with_context(|| {
                format!(
                    "Clone staging is retained for inspection at {}",
                    container.display()
                )
            })?
        };
    }
    let container_handle = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&anchored_container)
    {
        Ok(handle) => std::sync::Arc::new(handle),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "Cannot pin clone staging container; inspect {}",
                    container.display()
                )
            });
        }
    };
    let container_metadata = staging_try!(container_handle.metadata());
    staging_try!(verify_clone_identity(
        &container,
        container_metadata.dev(),
        container_metadata.ino()
    ));
    let pinned_container = staging_try!(pinned_clone_directory(
        &container_handle,
        Path::new("/proc")
    ));
    let stage = container.join("repository");
    let anchored_stage = pinned_container.join("repository");
    staging_try!(
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&anchored_stage)
            .context("Cannot create private clone staging repository")
    );
    let stage_handle = std::sync::Arc::new(staging_try!(
        fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&anchored_stage)
            .context("Cannot pin clone staging directory")
    ));
    let metadata = staging_try!(stage_handle.metadata());
    anyhow::ensure!(metadata.is_dir(), "Clone staging path is not a directory");
    staging_try!(verify_clone_identity(
        &stage,
        metadata.dev(),
        metadata.ino()
    ));
    let pinned_stage = staging_try!(pinned_clone_directory(&stage_handle, Path::new("/proc")));
    staging_try!(container_handle.sync_all());
    staging_try!(parent_handle.sync_all());
    let stored = StoredAccount {
        revision: 1,
        assignment: crate::AccountAssignment::Assigned {
            host: host.clone(),
            login: account.login.clone(),
            id: account.id,
        },
        association: None,
        source_remote: Some("origin".into()),
    };
    let mut intent = CloneIntent {
        id: clone_id,
        phase: "downloading".into(),
        url: url.to_owned(),
        certificate: None,
        stage: stage.clone(),
        destination: destination.clone(),
        device: metadata.dev(),
        inode: metadata.ino(),
        parent_identity: Some((parent_metadata.dev(), parent_metadata.ino())),
        account: stored.clone(),
    };
    let initial_registry = registry.clone();
    let initial_intent = intent.clone();
    crate::cancellation::blocking(move || initial_registry.begin_clone_intent(initial_intent))
        .await
        .with_context(|| {
            format!(
                "Clone staging could not be recorded; inspect {}",
                stage.display()
            )
        })?;
    let work = async {
        crate::git_credentials::clone::clone_no_checkout(
            &pinned_parent,
            url,
            &pinned_stage,
            &stage_handle,
            &container_handle,
            &container,
            options.branch.as_deref(),
            options.default_branch.as_deref(),
            chosen,
            options.report_progress,
        )
        .await?;
        verify_clone_identity(&stage, intent.device, intent.inode)?;
        let phase_registry = registry.clone();
        let phase_intent = intent.clone();
        crate::cancellation::blocking(move || {
            phase_registry.advance_clone_intent(&phase_intent, "downloading", "checkout", None)
        })
        .await?;
        intent.phase = "checkout".into();
        // Templates are installed after the provisional broker has stopped.
        let init =
            process::initialization_command(&pinned_stage, &args(&["init", "--quiet"])).await?;
        anyhow::ensure!(init.code == 0, "Cannot install clone templates");
        let mut repository = Repository::open_pinned(&pinned_stage).await?;
        anyhow::ensure!(repository.path() == stage, "Clone staging path changed");
        verify_clone_identity(&stage, intent.device, intent.inode)?;
        // All repository subprocesses and the temporary saved-record binding
        // use the held descriptor, even if the staging pathname is renamed.
        repository.path = pinned_stage.clone();
        let origin =
            process::run(&pinned_stage, &args(&["remote", "get-url", "--", "origin"])).await?;
        anyhow::ensure!(
            origin.code == 0 && origin.stdout.strip_suffix(b"\n") == Some(url.as_bytes()),
            "Clone source changed during checkout"
        );
        let temporary_registry = Registry::new(pinned_container.join("registry.json"))
            .with_owned_handles(vec![container_handle.clone(), stage_handle.clone()]);
        let save_registry = temporary_registry.clone();
        let save_stage = pinned_stage.clone();
        let save_account = stored.clone();
        let record = crate::cancellation::blocking(move || {
            save_registry.add_assigned_clone(save_stage, save_account)
        })
        .await?;
        let repository = repository.with_saved_record(temporary_registry, record.id);
        let _lock = repository.stash_lock().await?;
        let state = repository.account_state().await?;
        anyhow::ensure!(
            state.view.assignment == stored.assignment,
            "Clone account changed before checkout"
        );
        let basis = state.view.basis;
        let head = process::run(&pinned_stage, &args(&["rev-parse", "--verify", "HEAD"])).await?;
        if head.code != 0 {
            let symbolic =
                process::run(&pinned_stage, &args(&["symbolic-ref", "-q", "HEAD"])).await?;
            anyhow::ensure!(symbolic.code == 0, "Clone HEAD cannot be inspected");
            let reference = std::str::from_utf8(&symbolic.stdout)?
                .strip_suffix('\n')
                .context("Invalid clone HEAD")?;
            let exists = process::run(
                &pinned_stage,
                &args(&["show-ref", "--exists", "--", reference]),
            )
            .await?;
            anyhow::ensure!(exists.code == 2, "Clone HEAD target is invalid");
        } else {
            for entry in fs::read_dir(&pinned_stage)? {
                anyhow::ensure!(
                    entry?.file_name() == ".git",
                    "Clone staging directory changed before checkout"
                );
            }
            let operation = crate::Operation::RendererCheckoutDirect {
                branch: String::new(),
                basis: String::new(),
                allow_file_protocol: false,
            };
            let output = crate::git_credentials::clone::pinned_checkout(
                crate::git_credentials::scope(&repository, &operation, async {
                    repository.ensure_account_basis(&basis).await?;
                    process::run_checkout(&pinned_stage, &args(&["checkout", "HEAD"])).await
                }),
            )
            .await?;
            anyhow::ensure!(output.code == 0, "Clone checkout failed");
        }
        repository.ensure_account_basis(&basis).await?;
        drop(_lock);
        crate::git_credentials::clone::verify_clone_account(&pinned_stage, url, chosen).await?;
        let origin =
            process::run(&pinned_stage, &args(&["remote", "get-url", "--", "origin"])).await?;
        anyhow::ensure!(
            origin.code == 0 && origin.stdout.strip_suffix(b"\n") == Some(url.as_bytes()),
            "Clone source changed during checkout"
        );
        verify_clone_identity(&stage, intent.device, intent.inode)?;
        let certificate_handle = container_handle.clone();
        let certificate = crate::cancellation::blocking(move || {
            let path =
                pinned_clone_directory(&certificate_handle, Path::new("/proc"))?.join("repository");
            clone_certificate(&path)
        })
        .await?;
        let phase_registry = registry.clone();
        let phase_intent = intent.clone();
        let phase_certificate = certificate.clone();
        crate::cancellation::blocking(move || {
            phase_registry.advance_clone_intent(
                &phase_intent,
                "checkout",
                "ready",
                Some(phase_certificate),
            )
        })
        .await?;
        intent.phase = "ready".into();
        intent.certificate = Some(certificate);
        let conflict_registry = registry.clone();
        let conflict_intent = intent.clone();
        crate::cancellation::blocking(move || {
            conflict_registry.ensure_uncontested_clone(&conflict_intent)
        })
        .await?;
        let publish_intent = intent.clone();
        let publish_registry = registry.clone();
        crate::cancellation::blocking(move || {
            publish_clone_intent(&publish_registry, &publish_intent)
        })
        .await?;
        let save_registry = registry.clone();
        let save_intent = intent.clone();
        let record = crate::cancellation::blocking(move || {
            save_registry.finish_clone_intent(&save_intent, false)
        })
        .await?;
        let mut repository = Repository::open_pinned(&pinned_stage).await?;
        anyhow::ensure!(
            repository.path() == destination,
            "Published clone destination changed; inspect it before retrying"
        );
        repository.path = destination.clone();
        repository.retained_clone_staging = Some(container.clone());
        let repository = repository.with_saved_record(registry.clone(), record.id);
        // A same-user hook can replace entries in this exposed container. Do
        // not unlink them based on a raceable ownership check; retain it for
        // explicit inspection even after publication succeeds.
        drop(stage_handle);
        drop(container_handle);
        Ok::<_, anyhow::Error>(repository)
    }
    .await;
    let broker_home = container.join("broker-home");
    work.map_err(|_| anyhow::anyhow!("Clone may have completed; inspect recovery record {}, staging directory {}, and broker HOME {} if present before retrying", intent.id, stage.display(), broker_home.display()))
}

/// Resume only a finished staged clone, rechecking its actor, origin and
/// directory identity. Earlier phases are intentionally not replayed.
#[cfg(any(target_os = "linux", target_os = "android"))]
async fn recover_selected_clone_unavailable_implementation(
    registry: Registry,
    id: &str,
) -> Result<(SavedRepositoryRecord, PathBuf)> {
    use std::os::unix::fs::MetadataExt;
    let state = registry.read()?;
    let intent = state
        .clone_intents
        .iter()
        .find(|intent| intent.id == id)
        .cloned()
        .context("Clone recovery record not found")?;
    anyhow::ensure!(
        state
            .clone_intents
            .iter()
            .filter(|saved| saved.destination == intent.destination)
            .count()
            == 1,
        "Conflicting clone recovery records require manual inspection"
    );
    anyhow::ensure!(
        intent.phase == "ready",
        "Clone download or checkout was interrupted; inspect its staging directory before retrying"
    );
    let expected_parent = intent
        .parent_identity
        .context("Older clone recovery requires manual inspection")?;
    let parent = open_clone_parent(
        intent
            .destination
            .parent()
            .context("Invalid clone destination")?,
    )?;
    let metadata = parent.metadata()?;
    anyhow::ensure!(
        (metadata.dev(), metadata.ino()) == expected_parent,
        "Clone destination parent changed; inspect it before recovery"
    );
    let candidate = match fs::symlink_metadata(&intent.stage) {
        Ok(_) => &intent.stage,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => &intent.destination,
        Err(error) => return Err(error).context("Cannot inspect clone staging directory"),
    };
    let candidate_handle = open_clone_parent(candidate)?;
    let metadata = candidate_handle.metadata()?;
    anyhow::ensure!(
        (metadata.dev(), metadata.ino()) == (intent.device, intent.inode),
        "Clone recovery directory changed"
    );
    let pinned_candidate = pinned_clone_directory(&candidate_handle, Path::new("/proc"))?;
    let candidate_parent = open_clone_parent(
        candidate
            .parent()
            .context("Invalid clone recovery parent")?,
    )?;
    let certificate_name = candidate
        .file_name()
        .context("Invalid clone recovery name")?
        .to_owned();
    let certificate_intent = intent.clone();
    crate::cancellation::blocking(move || {
        let path =
            pinned_clone_directory(&candidate_parent, Path::new("/proc"))?.join(certificate_name);
        verify_clone_certificate(&path, &certificate_intent)
    })
    .await?;
    let repository = Repository::open_pinned(&pinned_candidate).await?;
    anyhow::ensure!(
        repository.path() == candidate,
        "Clone recovery path changed"
    );
    let origin = process::run(
        &pinned_candidate,
        &args(&["remote", "get-url", "--", "origin"]),
    )
    .await?;
    anyhow::ensure!(
        origin.code == 0 && origin.stdout.strip_suffix(b"\n") == Some(intent.url.as_bytes()),
        "Clone recovery source changed"
    );
    let crate::AccountAssignment::Assigned { host, login, id } = &intent.account.assignment else {
        bail!("Clone recovery account is invalid")
    };
    let account = crate::github_api::ChosenAccount {
        host,
        login,
        id: Some(*id),
    };
    crate::git_credentials::clone::verify_clone_account(&pinned_candidate, &intent.url, account)
        .await?;
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .context("Cannot inspect the user's home directory")?;
    let home = fs::canonicalize(home).context("Cannot inspect the user's home directory")?;
    ensure_clone_destination_outside_sensitive_home(&intent.destination, &home)?;
    let publish_intent = intent.clone();
    let publish_registry = registry.clone();
    crate::cancellation::blocking(move || publish_clone_intent(&publish_registry, &publish_intent)).await.map_err(|_| anyhow::anyhow!("Clone may have been published; inspect its destination and recovery record before retrying"))?;
    let save_registry = registry.clone();
    let save_intent = intent.clone();
    let record = crate::cancellation::blocking(move || save_registry.finish_clone_intent(&save_intent, false)).await.map_err(|_| anyhow::anyhow!("Clone may have been published; inspect its destination and recovery record before retrying"))?;
    // A recovered process no longer holds the original staging-container
    // descriptor. Leave its metadata for explicit inspection rather than
    // deleting files through a potentially replaced pathname.
    let retained = intent
        .stage
        .parent()
        .context("Invalid clone staging path")?
        .to_owned();
    Ok((record, retained))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub async fn recover_selected_clone(registry: Registry, id: &str) -> Result<SavedRepositoryRecord> {
    recover_selected_clone_with_staging(registry, id)
        .await
        .map(|(record, _)| record)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub async fn recover_selected_clone_with_staging(
    registry: Registry,
    id: &str,
) -> Result<(SavedRepositoryRecord, PathBuf)> {
    recover_selected_clone_unavailable_implementation(registry, id).await
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub async fn recover_selected_clone(
    _registry: Registry,
    _id: &str,
) -> Result<SavedRepositoryRecord> {
    bail!("Selected-account clone recovery is unavailable on this platform")
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub async fn recover_selected_clone_with_staging(
    _registry: Registry,
    _id: &str,
) -> Result<(SavedRepositoryRecord, PathBuf)> {
    bail!("Selected-account clone recovery is unavailable on this platform")
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub async fn clone_repository_selected(
    registry: Registry,
    url: &str,
    path: &Path,
    options: CloneOptions,
    account: CloneAccount,
) -> Result<Repository> {
    clone_repository_selected_unavailable_implementation(registry, url, path, options, account)
        .await
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub async fn clone_repository_selected(
    _registry: Registry,
    _url: &str,
    _path: &Path,
    _options: CloneOptions,
    _account: CloneAccount,
) -> Result<Repository> {
    bail!("Selected-account clone is unavailable on this platform")
}

pub async fn clone_repository_with_options(
    url: &str,
    path: &Path,
    options: CloneOptions,
) -> Result<Repository> {
    if url.is_empty()
        || url.starts_with('-')
        || url.contains(['\0', '\n', '\r'])
        || url.starts_with("ext::")
        || url.starts_with("ext.exe::")
    {
        bail!("invalid clone URL");
    }
    for value in [&options.branch, &options.default_branch]
        .into_iter()
        .flatten()
    {
        anyhow::ensure!(
            !value.is_empty()
                && value.len() <= 4096
                && !value.starts_with('-')
                && !value.chars().any(char::is_control),
            "invalid clone branch"
        );
    }
    let home = clone_home_directory().context("Cannot determine the user's home directory")?;
    #[cfg(windows)]
    let app_data = std::env::var_os("APPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    #[cfg(not(windows))]
    let app_data: Option<PathBuf> = None;
    ensure_ordinary_clone_destination(path, Some(&home), app_data.as_deref())?;
    let destination = prepare_destination(path)?;
    // Check the resolved destination again after creating missing parents. Git
    // still owns the final operation and independent writers can race it.
    ensure_ordinary_clone_destination(&destination, Some(&home), app_data.as_deref())?;
    let parent = destination
        .parent()
        .context("repository needs a parent directory")?;
    let mut command = args(&[
        "-c",
        "protocol.ext.allow=never",
        "-c",
        "protocol.ext.exe.allow=never",
    ]);
    if let Some(default_branch) = &options.default_branch {
        command.extend(args(&[
            "-c",
            &format!("init.defaultBranch={default_branch}"),
        ]));
    }
    command.push("clone".into());
    if options.report_progress {
        command.push("--progress".into());
    }
    if options.recursive {
        command.push("--recursive".into());
    }
    if let Some(branch) = &options.branch {
        command.push(format!("--branch={branch}").into());
    }
    command.extend(args(&["--", url]));
    command.push(destination.as_os_str().to_owned());
    let output = process::run_clone(parent, &command, options.report_progress).await?;
    if output.code != 0 {
        bail!("{}", clone_failure_message(&output));
    }
    Repository::open(&destination).await.map_err(|_| {
        anyhow::anyhow!("Clone completed, but its repository could not be opened; inspect the destination before retrying")
    })
}

fn clone_home_directory() -> Option<PathBuf> {
    #[cfg(windows)]
    let configured = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let configured = std::env::var_os("HOME");
    configured
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            #[allow(deprecated)]
            std::env::home_dir()
        })
}

/// Resolve existing links while preserving the names of missing descendants.
fn canonical_clone_candidate(path: &Path) -> Result<PathBuf> {
    Ok(resolve_clone_candidate(path)?.0)
}

/// Also retain missing paths traversed before `..`: `create_dir_all` can
/// create these even when they are not part of the final destination.
fn resolve_clone_candidate(path: &Path) -> Result<(PathBuf, Vec<PathBuf>)> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut resolved = PathBuf::new();
    let mut traversed_missing = Vec::new();
    for component in absolute.components() {
        match component {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {
                resolved.push(component.as_os_str());
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !resolved.exists() {
                    traversed_missing.push(resolved.clone());
                }
                if !resolved.pop() {
                    anyhow::ensure!(resolved.has_root(), "Cannot resolve clone path parent");
                }
            }
            std::path::Component::Normal(name) => {
                let candidate = resolved.join(name);
                match fs::canonicalize(&candidate) {
                    Ok(path) => resolved = crate::repository_discovery::display_canonical(path),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        resolved.push(name);
                    }
                    Err(error) => return Err(error).context("Cannot inspect clone destination"),
                }
            }
        }
    }
    Ok((resolved, traversed_missing))
}

/// Preserve aliases at every resolvable ancestor as well as the final target.
/// This catches a differently-cased sensitive name that is itself a symlink.
fn clone_candidate_aliases(path: &Path) -> Result<Vec<PathBuf>> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let (destination, traversed_missing) = resolve_clone_candidate(&absolute)?;
    let mut candidates = vec![destination];
    candidates.extend(traversed_missing);
    for ancestor in absolute.ancestors() {
        if let Ok(resolved) = fs::canonicalize(ancestor) {
            let suffix = absolute.strip_prefix(ancestor)?;
            let candidate = crate::repository_discovery::normalize(
                &crate::repository_discovery::display_canonical(resolved).join(suffix),
            );
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
    }
    Ok(candidates)
}

fn clone_path_starts_with(path: &Path, base: &Path) -> bool {
    // The existing renderer compares lowercased paths on every platform.
    // Preserve byte-exact comparison when an OS path is not valid Unicode.
    let mut components = path.components();
    base.components().all(|base| {
        components.next().is_some_and(|part| {
            match (part.as_os_str().to_str(), base.as_os_str().to_str()) {
                (Some(part), Some(base)) => part.to_lowercase() == base.to_lowercase(),
                _ => part.as_os_str() == base.as_os_str(),
            }
        })
    })
}

/// Reject the same sensitive destinations as the original renderer, including
/// existing aliases of those directories. No directories are created here.
fn ensure_ordinary_clone_destination(
    path: &Path,
    home: Option<&Path>,
    app_data: Option<&Path>,
) -> Result<()> {
    let destinations = clone_candidate_aliases(path)?;
    if let Some(home) = home {
        let home = canonical_clone_candidate(home)?;
        for destination in &destinations {
            anyhow::ensure!(
                !(clone_path_starts_with(destination, &home)
                    && clone_path_starts_with(&home, destination)),
                "Clone destination targets a sensitive system location"
            );
        }
        for name in [".ssh", ".gnupg", ".config", ".gitconfig"] {
            let protected = home.join(name);
            // An inaccessible protected target should not prevent cloning to
            // an unrelated directory; still reject its lexical path.
            let resolved = canonical_clone_candidate(&protected).ok();
            for destination in &destinations {
                anyhow::ensure!(
                    !clone_path_starts_with(destination, &protected)
                        && !resolved
                            .as_ref()
                            .is_some_and(|resolved| clone_path_starts_with(destination, resolved)),
                    "Clone destination targets a sensitive system location"
                );
            }
        }
    }
    if let Some(app_data) = app_data {
        let app_data = canonical_clone_candidate(app_data)?;
        for destination in &destinations {
            anyhow::ensure!(
                !clone_path_starts_with(destination, &app_data),
                "Clone destination targets a sensitive system location"
            );
        }
    }
    Ok(())
}

/// Clone output may contain raw URLs (including credentials added by a Git
/// rewrite) or remote-controlled text. Only fixed descriptions cross the API.
pub(crate) fn clone_failure_message(output: &process::RawOutput) -> &'static str {
    match crate::fetch_error::classify(&output.stderr, &output.stdout, None) {
        Some(
            "https_authentication_failed"
            | "ssh_authentication_failed"
            | "ssh_permission_denied"
            | "ssh_key_unverified",
        ) => "Clone authentication failed. Check your credentials and access to the repository.",
        Some("host_resolution_failed" | "host_down") => {
            "The remote host could not be reached. Check your connection and remote URL."
        }
        Some("remote_disconnection") => {
            "The remote disconnected during clone. Check your connection and try again."
        }
        Some(
            "https_repository_not_found"
            | "ssh_repository_not_found"
            | "remote_repository_unavailable",
        ) => {
            "The remote repository could not be accessed. Check that it exists and that you have access."
        }
        Some("bad_config_value") => "Clone failed because a Git configuration value is invalid.",
        _ if String::from_utf8_lossy(&output.stderr).lines().any(|line| {
            line.starts_with("fatal: repository '") && line.ends_with("' does not exist")
        }) =>
        {
            "The remote repository could not be accessed. Check that it exists and that you have access."
        }
        _ => "Clone failed. Check the remote and destination before retrying.",
    }
}

fn prepare_destination(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    if absolute.exists() && (!absolute.is_dir() || fs::read_dir(&absolute)?.next().is_some()) {
        bail!("destination must be absent or an empty directory");
    }
    let parent = absolute
        .parent()
        .context("destination needs a parent directory")?;
    fs::create_dir_all(parent)?;
    let name = absolute
        .file_name()
        .context("destination needs a directory name")?;
    Ok(parent.canonicalize()?.join(name))
}

/// Keep native configuration independent of Electron and available without HOME on Windows.
fn config_directory(mut env: impl FnMut(&str) -> Option<std::ffi::OsString>) -> Result<PathBuf> {
    let mut nonempty = |key: &str| env(key).filter(|value| !value.is_empty());
    let base = match nonempty("XDG_CONFIG_HOME") {
        Some(path) => PathBuf::from(path),
        None => {
            #[cfg(target_os = "windows")]
            {
                PathBuf::from(
                    nonempty("LOCALAPPDATA")
                        .context("LOCALAPPDATA is not set; provide --config")?,
                )
            }
            #[cfg(not(target_os = "windows"))]
            {
                let home =
                    PathBuf::from(nonempty("HOME").context("HOME is not set; provide --config")?);
                #[cfg(target_os = "macos")]
                {
                    home.join("Library/Application Support")
                }
                #[cfg(not(target_os = "macos"))]
                {
                    home.join(".config")
                }
            }
        }
    };
    if !base.is_absolute() {
        bail!("configuration directory must be absolute; provide --config");
    }
    Ok(base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn ordinary_clone_rejects_sensitive_destinations_before_creation() {
        let fixture = RegistryFixture::new();
        let home = fixture.directory.join("home");
        let app_data = fixture.directory.join("app-data");
        fs::create_dir(&home).unwrap();
        fs::create_dir(&app_data).unwrap();
        for path in [
            home.clone(),
            home.join(".ssh/repository"),
            home.join(".SSH/repository"),
            home.join(".gnupg/repository"),
            home.join(".config/git/repository"),
            home.join(".cOnFiG/git/repository"),
            home.join(".gitconfig/repository"),
            app_data.join("repository"),
        ] {
            assert!(
                ensure_ordinary_clone_destination(&path, Some(&home), Some(&app_data)).is_err(),
                "{}",
                path.display()
            );
        }
        assert!(
            ensure_ordinary_clone_destination(
                &home.join("projects/repository"),
                Some(&home),
                Some(&app_data)
            )
            .is_ok()
        );
        assert!(
            ensure_ordinary_clone_destination(
                &home.join(".ssh-backup/repository"),
                Some(&home),
                Some(&app_data)
            )
            .is_ok()
        );
        assert!(!home.join(".ssh").exists());
        assert!(!home.join("projects").exists());
    }

    #[test]
    fn ordinary_clone_resolves_missing_parent_components() {
        let fixture = RegistryFixture::new();
        let home = fixture.directory.join("home");
        fs::create_dir(&home).unwrap();
        let path = home.join("new/../repository");
        assert_eq!(
            canonical_clone_candidate(&path).unwrap(),
            home.join("repository")
        );
        assert!(ensure_ordinary_clone_destination(&path, Some(&home), None).is_ok());
        assert!(!home.join("new").exists());
        assert!(!home.join("repository").exists());

        // Git's destination preparation may create these parents before
        // traversing `..`, even though the final path is outside `.ssh`.
        let through_sensitive = home.join(".ssh/new/../../repository");
        assert!(ensure_ordinary_clone_destination(&through_sensitive, Some(&home), None).is_err());
        assert!(!home.join(".ssh").exists());
    }

    #[cfg(unix)]
    #[test]
    fn ordinary_clone_resolves_symlinks_before_parent_components() {
        use std::os::unix::fs::symlink;
        let fixture = RegistryFixture::new();
        let outside = fixture.directory.join("outside");
        let target = outside.join("target");
        fs::create_dir_all(&target).unwrap();
        let alias = fixture.directory.join("alias");
        symlink(&target, &alias).unwrap();
        let candidate = alias.join("../repository");
        assert_eq!(
            canonical_clone_candidate(&candidate).unwrap(),
            outside.join("repository")
        );
    }

    #[cfg(unix)]
    #[test]
    fn ordinary_clone_rejects_symlink_aliases_of_sensitive_directories() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let fixture = RegistryFixture::new();
        let home = fixture.directory.join("home");
        let protected = fixture.directory.join("protected");
        fs::create_dir(&home).unwrap();
        fs::create_dir(&protected).unwrap();
        fs::create_dir(fixture.directory.join("elsewhere")).unwrap();
        symlink(&protected, home.join(".ssh")).unwrap();
        symlink(&protected, fixture.directory.join("alias")).unwrap();
        symlink(&home, fixture.directory.join("home-alias")).unwrap();
        symlink(fixture.directory.join("elsewhere"), home.join(".SSH")).unwrap();
        symlink(
            fixture.directory.join("elsewhere"),
            fixture.directory.join("appdata-alias"),
        )
        .unwrap();
        assert!(
            ensure_ordinary_clone_destination(
                &fixture.directory.join("alias/new/repo"),
                Some(&home),
                None
            )
            .is_err()
        );
        assert!(!protected.join("new").exists());
        assert!(
            ensure_ordinary_clone_destination(
                &fixture.directory.join("home-alias/.SSH/repo"),
                Some(&home),
                None
            )
            .is_err()
        );
        assert!(
            ensure_ordinary_clone_destination(
                &fixture.directory.join("elsewhere/repo"),
                None,
                Some(&fixture.directory.join("appdata-alias")),
            )
            .is_err()
        );
        // An inaccessible destination must not be treated as a missing path.
        // Root can traverse mode-000 directories, so exercise this when the
        // current user is subject to normal Unix permission checks.
        if unsafe { libc::geteuid() } != 0 {
            let inaccessible = fixture.directory.join("inaccessible");
            fs::create_dir(&inaccessible).unwrap();
            symlink(&inaccessible, fixture.directory.join("inaccessible-alias")).unwrap();
            fs::set_permissions(&inaccessible, fs::Permissions::from_mode(0o000)).unwrap();
            let result = ensure_ordinary_clone_destination(
                &fixture.directory.join("inaccessible-alias/repo"),
                Some(&home),
                None,
            );
            fs::set_permissions(&inaccessible, fs::Permissions::from_mode(0o700)).unwrap();
            assert!(result.is_err());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ordinary_clone_checks_sensitive_home_before_running_git() {
        use std::process::Command;
        const CHILD: &str = "DESKTOP_TEST_ORDINARY_CLONE_HOME";
        if let Some(root) = std::env::var_os(CHILD) {
            let root = PathBuf::from(root);
            let source = root.join("source.git");
            assert!(
                Command::new("git")
                    .args(["init", "--bare", "-q"])
                    .arg(&source)
                    .status()
                    .unwrap()
                    .success()
            );
            let protected = root.join(".ssh/new/repository");
            let error = clone_repository(source.to_str().unwrap(), &protected)
                .await
                .err()
                .expect("sensitive path must fail");
            assert!(error.to_string().contains("sensitive system location"));
            assert!(!root.join(".ssh").exists());
            let destination = root.join("projects/repository");
            clone_repository(source.to_str().unwrap(), &destination)
                .await
                .unwrap();
            assert!(destination.join(".git").is_dir());
            let through_missing_parent = root.join("new/../second");
            clone_repository(source.to_str().unwrap(), &through_missing_parent)
                .await
                .unwrap();
            assert!(root.join("second/.git").is_dir());
            return;
        }
        let fixture = RegistryFixture::new();
        let result = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "registry::tests::ordinary_clone_checks_sensitive_home_before_running_git",
                "--nocapture",
            ])
            .env_clear()
            .envs(std::env::vars_os().filter(|(key, _)| !key.to_string_lossy().starts_with("GIT_")))
            .env(CHILD, &fixture.directory)
            .env("HOME", &fixture.directory)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_ALLOW_PROTOCOL", "file")
            .env(
                "GIT_CONFIG_GLOBAL",
                fixture.directory.join("missing-config"),
            )
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[tokio::test]
    #[ignore = "requires the offline selected-account HTTPS fixture"]
    async fn child_fd_selected_clone_fixture() {
        let destination = PathBuf::from(
            std::env::var_os("DESKTOP_TEST_CLONE_DESTINATION").expect("fixture destination"),
        );
        let registry_path = PathBuf::from(
            std::env::var_os("DESKTOP_TEST_CLONE_REGISTRY").expect("fixture registry"),
        );
        let url = std::env::var("DESKTOP_TEST_CLONE_URL").expect("fixture URL");
        let host = std::env::var("DESKTOP_TEST_CLONE_HOST").expect("fixture host");
        let registry = Registry::new(registry_path);
        let repository = clone_repository_selected_unavailable_implementation(
            registry.clone(),
            &url,
            &destination,
            CloneOptions {
                branch: None,
                default_branch: Some("main".into()),
                recursive: false,
                report_progress: false,
            },
            CloneAccount {
                host: host.clone(),
                login: "AliceCase".into(),
                id: 101,
            },
        )
        .await
        .unwrap();
        assert_eq!(repository.path(), destination);
        assert_eq!(fs::read(destination.join("file")).unwrap(), b"base\n");
        let state = registry.read().unwrap();
        assert!(state.selected.is_none());
        let record = state.record_for_path(&destination).unwrap();
        assert_eq!(
            record.account.assignment,
            crate::AccountAssignment::Assigned {
                host,
                login: "AliceCase".into(),
                id: 101
            }
        );
        let retained = destination
            .parent()
            .unwrap()
            .join(format!(".desktop-clone-{}", record.id));
        assert_eq!(
            repository.retained_clone_staging(),
            Some(retained.as_path())
        );
        assert!(retained.join("broker-home").is_dir());
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[tokio::test]
    #[ignore = "requires the offline selected-account HTTPS fixture"]
    async fn child_fd_selected_clone_recovery_fixture() {
        let destination = PathBuf::from(
            std::env::var_os("DESKTOP_TEST_CLONE_DESTINATION").expect("fixture destination"),
        );
        let registry_path = PathBuf::from(
            std::env::var_os("DESKTOP_TEST_CLONE_REGISTRY").expect("fixture registry"),
        );
        let started = PathBuf::from(
            std::env::var_os("DESKTOP_TEST_CLONE_STARTED").expect("fixture start gate"),
        );
        let release = PathBuf::from(
            std::env::var_os("DESKTOP_TEST_CLONE_RELEASE").expect("fixture release gate"),
        );
        let url = std::env::var("DESKTOP_TEST_CLONE_URL").expect("fixture URL");
        let host = std::env::var("DESKTOP_TEST_CLONE_HOST").expect("fixture host");
        let obstruction = destination.clone();
        let blocker = tokio::spawn(async move {
            tokio::time::timeout(std::time::Duration::from_secs(30), async {
                while !started.exists() {
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
            })
            .await
            .unwrap();
            fs::create_dir(&obstruction).unwrap();
            fs::write(obstruction.join("user-data"), "preserve").unwrap();
            fs::write(release, "release").unwrap();
        });
        let registry = Registry::new(registry_path);
        let result = clone_repository_selected_unavailable_implementation(
            registry.clone(),
            &url,
            &destination,
            CloneOptions {
                branch: None,
                default_branch: Some("main".into()),
                recursive: false,
                report_progress: false,
            },
            CloneAccount {
                host: host.clone(),
                login: "AliceCase".into(),
                id: 101,
            },
        )
        .await;
        blocker.await.unwrap();
        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(destination.join("user-data")).unwrap(),
            "preserve"
        );
        let pending = registry.read().unwrap().clone_intents;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].phase, "ready");
        fs::remove_file(destination.join("user-data")).unwrap();
        fs::remove_dir(&destination).unwrap();
        if std::env::var_os("DESKTOP_TEST_CLONE_PREPARE_RECOVERY_ONLY").is_some() {
            return;
        }
        let (record, retained) =
            recover_selected_clone_unavailable_implementation(registry.clone(), &pending[0].id)
                .await
                .unwrap();
        assert_eq!(record.path, destination);
        assert_eq!(retained, pending[0].stage.parent().unwrap());
        assert_eq!(fs::read(destination.join("file")).unwrap(), b"base\n");
        let state = registry.read().unwrap();
        assert!(state.selected.is_none());
        assert!(state.clone_intents.is_empty());
        assert_eq!(
            record.account.assignment,
            crate::AccountAssignment::Assigned {
                host,
                login: "AliceCase".into(),
                id: 101
            }
        );
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[tokio::test]
    #[ignore = "requires a clone recovery journal from the offline HTTPS fixture"]
    async fn child_fd_changed_home_recovery_fixture() {
        let registry = Registry::new(PathBuf::from(
            std::env::var_os("DESKTOP_TEST_CLONE_REGISTRY").expect("fixture registry"),
        ));
        let pending = registry.read().unwrap().clone_intents;
        assert_eq!(pending.len(), 1);
        let intent = &pending[0];
        let error = recover_selected_clone_unavailable_implementation(registry.clone(), &intent.id)
            .await
            .err()
            .expect("changed HOME must prevent recovery");
        assert!(error.to_string().contains("sensitive system location"));
        assert!(!intent.destination.exists());
        assert!(intent.stage.is_dir());
        assert!(
            registry
                .read()
                .unwrap()
                .record_for_path(&intent.destination)
                .is_none()
        );
        assert_eq!(registry.read().unwrap().clone_intents.len(), 1);
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[tokio::test]
    #[ignore = "requires the offline selected-account HTTPS fixture"]
    async fn child_fd_clone_admission_failure_fixture() {
        let destination = PathBuf::from(
            std::env::var_os("DESKTOP_TEST_CLONE_DESTINATION").expect("fixture destination"),
        );
        let registry_path = PathBuf::from(
            std::env::var_os("DESKTOP_TEST_CLONE_REGISTRY").expect("fixture registry"),
        );
        let url = std::env::var("DESKTOP_TEST_CLONE_URL").expect("fixture URL");
        let host = std::env::var("DESKTOP_TEST_CLONE_HOST").expect("fixture host");
        let error = clone_repository_selected_unavailable_implementation(
            Registry::new(registry_path),
            &url,
            &destination,
            CloneOptions {
                branch: None,
                default_branch: Some("main".into()),
                recursive: false,
                report_progress: false,
            },
            CloneAccount {
                host,
                login: "AliceCase".into(),
                id: 101,
            },
        )
        .await
        .err()
        .expect("fixture registry is a directory");
        let parent = destination.parent().unwrap();
        let retained = fs::read_dir(parent)
            .unwrap()
            .filter_map(Result::ok)
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".desktop-clone-")
            })
            .expect("retained staging container");
        assert!(
            format!("{error:#}")
                .contains(&retained.path().join("repository").display().to_string())
        );
        assert!(retained.path().join("repository").is_dir());
        assert!(!destination.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn clone_options_preserve_branch_recursive_and_default_behavior() {
        use std::process::Command;
        const CHILD: &str = "DESKTOP_TEST_CLONE_OPTIONS_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let fixture = RegistryFixture::new();
            let result = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "registry::tests::clone_options_preserve_branch_recursive_and_default_behavior",
                    "--nocapture",
                ])
                .env(CHILD, &fixture.directory)
                .env("HOME", &fixture.directory)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env(
                    "GIT_CONFIG_GLOBAL",
                    fixture.directory.join("missing-config"),
                )
                .env("GIT_ALLOW_PROTOCOL", "file")
                .env_remove("GIT_CONFIG_COUNT")
                .env_remove("GIT_CONFIG_PARAMETERS")
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            return;
        }
        let root = PathBuf::from(std::env::var_os(CHILD).unwrap());
        let git = |dir: &Path, arguments: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(arguments)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        let source = root.join("source");
        let sub = root.join("sub");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&sub).unwrap();
        for dir in [&source, &sub] {
            git(dir, &["init", "-q", "-b", "main"]);
            git(dir, &["config", "user.name", "Test"]);
            git(dir, &["config", "user.email", "test@example.invalid"]);
            git(dir, &["config", "commit.gpgsign", "false"]);
            fs::write(dir.join("file"), "initial\n").unwrap();
            git(dir, &["add", "file"]);
            git(dir, &["commit", "-qm", "initial"]);
        }
        git(
            &source,
            &["submodule", "add", "-q", sub.to_str().unwrap(), "nested"],
        );
        git(&source, &["commit", "-qm", "submodule"]);
        git(&source, &["checkout", "-qb", "feature"]);
        fs::write(source.join("feature"), "selected\n").unwrap();
        git(&source, &["add", "feature"]);
        git(&source, &["commit", "-qm", "feature"]);
        git(&source, &["checkout", "-q", "main"]);

        let recursive = root.join("recursive");
        clone_repository_with_options(
            source.to_str().unwrap(),
            &recursive,
            CloneOptions {
                branch: Some("feature".into()),
                default_branch: Some("fallback".into()),
                recursive: true,
                report_progress: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            git(&recursive, &["symbolic-ref", "--short", "HEAD"]),
            "feature"
        );
        assert_eq!(
            fs::read_to_string(recursive.join("nested/file")).unwrap(),
            "initial\n"
        );
        let ordinary = root.join("ordinary");
        clone_repository(source.to_str().unwrap(), &ordinary)
            .await
            .unwrap();
        assert!(
            !ordinary.join("nested/file").exists(),
            "legacy endpoint remains non-recursive"
        );

        let empty = root.join("empty.git");
        git(&root, &["init", "--bare", "-q", empty.to_str().unwrap()]);
        let legacy_empty = root.join("empty-legacy");
        git(
            &root,
            &[
                "-c",
                "init.defaultBranch=fallback",
                "clone",
                "--",
                empty.to_str().unwrap(),
                legacy_empty.to_str().unwrap(),
            ],
        );
        let empty_clone = root.join("empty-clone");
        clone_repository_with_options(
            empty.to_str().unwrap(),
            &empty_clone,
            CloneOptions {
                default_branch: Some("fallback".into()),
                ..CloneOptions::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            git(&empty_clone, &["symbolic-ref", "--short", "HEAD"]),
            git(&legacy_empty, &["symbolic-ref", "--short", "HEAD"])
        );
        let invalid = root.join("missing-parent").join("invalid");
        assert!(
            clone_repository_with_options(
                source.to_str().unwrap(),
                &invalid,
                CloneOptions {
                    branch: Some("--upload-pack=bad".into()),
                    ..CloneOptions::default()
                }
            )
            .await
            .is_err()
        );
        assert!(!invalid.parent().unwrap().exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn recursive_clone_blocks_rewritten_external_submodule_helper() {
        use std::{os::unix::fs::PermissionsExt, process::Command};
        const CHILD: &str = "DESKTOP_TEST_CLONE_SUBMODULE_PROTOCOL_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let fixture = RegistryFixture::new();
            let root = fixture.directory.clone();
            let helper = root.join("external-helper");
            fs::write(
                &helper,
                "#!/bin/sh\n: > \"$DESKTOP_CLONE_TEST_MARKER\"\nexit 1\n",
            )
            .unwrap();
            fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
            let config = root.join("gitconfig");
            assert!(
                Command::new("git")
                    .args(["config", "--file"])
                    .arg(&config)
                    .arg(format!("url.ext::{}.insteadOf", helper.display()))
                    .arg("https://unsafe.invalid/repo")
                    .status()
                    .unwrap()
                    .success()
            );
            let result = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "registry::tests::recursive_clone_blocks_rewritten_external_submodule_helper",
                    "--nocapture",
                ])
                .env(CHILD, &root)
                .env("HOME", &root)
                .env("GIT_CONFIG_GLOBAL", &config)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_ALLOW_PROTOCOL", "file:ext")
                .env("DESKTOP_CLONE_TEST_MARKER", root.join("executed"))
                .env_remove("GIT_CONFIG_COUNT")
                .env_remove("GIT_CONFIG_PARAMETERS")
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert!(!root.join("executed").exists());
            return;
        }
        let root = PathBuf::from(std::env::var_os(CHILD).unwrap());
        let git = |dir: &Path, arguments: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(arguments)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        let source = root.join("source");
        let sub = root.join("sub");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&sub).unwrap();
        for dir in [&source, &sub] {
            git(dir, &["init", "-q", "-b", "main"]);
            git(dir, &["config", "user.name", "Test"]);
            git(dir, &["config", "user.email", "test@example.invalid"]);
            git(dir, &["config", "commit.gpgsign", "false"]);
            git(dir, &["commit", "--allow-empty", "-qm", "initial"]);
        }
        git(
            &source,
            &["submodule", "add", "-q", sub.to_str().unwrap(), "nested"],
        );
        git(
            &source,
            &[
                "config",
                "-f",
                ".gitmodules",
                "submodule.nested.url",
                "https://unsafe.invalid/repo",
            ],
        );
        git(&source, &["add", ".gitmodules", "nested"]);
        git(&source, &["commit", "-qm", "submodule"]);
        let control = Command::new("git")
            .args(["clone", "--recursive", "--"])
            .arg(&source)
            .arg(root.join("control"))
            .output()
            .unwrap();
        assert!(!control.status.success());
        assert!(
            root.join("executed").exists(),
            "control must run the fixture helper"
        );
        fs::remove_file(root.join("executed")).unwrap();
        assert!(
            clone_repository_with_options(
                source.to_str().unwrap(),
                &root.join("guarded"),
                CloneOptions {
                    recursive: true,
                    ..CloneOptions::default()
                }
            )
            .await
            .is_err()
        );
        assert!(
            !root.join("executed").exists(),
            "guarded recursive clone must not run external helper"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn clone_and_initialize_report_open_failures_in_their_own_context() {
        use std::{os::unix::fs::PermissionsExt, process::Command};
        const CHILD: &str = "DESKTOP_TEST_REPOSITORY_OPEN_ERROR_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let fixture = RegistryFixture::new();
            let root = fixture.directory.clone();
            let bin = root.join("bin");
            fs::create_dir(&bin).unwrap();
            let git = bin.join("git");
            fs::write(&git, "#!/bin/sh\ncase \" $* \" in\n  *\" config --get init.defaultbranch \"*) exit 1;;\n  *\" rev-parse --show-toplevel \"*) echo synthetic-open-secret >&2; exit 128;;\n  *) exit 0;;\nesac\n").unwrap();
            fs::set_permissions(&git, fs::Permissions::from_mode(0o700)).unwrap();
            let mut path = std::ffi::OsString::from(bin);
            path.push(":");
            path.push(std::env::var_os("PATH").unwrap_or_default());
            let result = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "registry::tests::clone_and_initialize_report_open_failures_in_their_own_context", "--nocapture"])
                .env(CHILD, &root)
                .env("PATH", path)
                .env("HOME", &root)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env_remove("GIT_CONFIG_COUNT")
                .env_remove("GIT_CONFIG_PARAMETERS")
                .output().unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            return;
        }
        let root = PathBuf::from(std::env::var_os(CHILD).unwrap());
        let initial = root.join("initial");
        let cloned = root.join("cloned");
        fs::create_dir(&initial).unwrap();
        fs::create_dir(&cloned).unwrap();
        let error = initialize(&initial)
            .await
            .err()
            .expect("initialize open must fail");
        assert!(error.to_string().contains("not a working tree"));
        assert!(!error.to_string().contains("Clone completed"));
        let error = clone_repository("https://example.invalid/repo", &cloned)
            .await
            .err()
            .expect("clone open must fail");
        assert!(error.to_string().contains("Clone completed"));
        assert!(!format!("{error:?}").contains("synthetic-open-secret"));
    }

    #[test]
    fn clone_error_messages_are_fixed_and_useful() {
        let classify = |stderr: &str| {
            clone_failure_message(&process::RawOutput {
                code: 128,
                stdout: Vec::new(),
                stderr: stderr.as_bytes().to_vec(),
            })
        };
        for (diagnostic, expected) in [
            (
                "fatal: Authentication failed for 'https://user:secret@example.invalid/repo'",
                "Clone authentication failed",
            ),
            (
                "fatal: unable to access 'https://secret.example/repo': Could not resolve host: secret.example",
                "remote host could not be reached",
            ),
            (
                "fatal: repository '/private/secret' does not exist",
                "remote repository could not be accessed",
            ),
            ("fatal: unexpected secret diagnostics", "Clone failed"),
        ] {
            let message = classify(diagnostic);
            assert!(message.contains(expected), "{message}");
            assert!(!message.contains("secret"));
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn clone_errors_do_not_expose_direct_or_rewritten_secrets() {
        use std::{net::TcpListener, process::Command, time::Duration};
        const CHILD: &str = "DESKTOP_TEST_CLONE_ERROR_CHILD";
        const SECRET: &str = "synthetic-clone-secret";
        if std::env::var_os(CHILD).is_none() {
            let fixture = RegistryFixture::new();
            let root = fixture.directory.clone();
            let config = root.join("gitconfig");
            // Git's real diagnostic includes the rewritten path. The sentinel
            // stands in for sensitive material introduced by user config.
            let rewritten = format!("file://{}", root.join(SECRET).display());
            let setting = format!("url.{rewritten}.insteadOf");
            assert!(
                Command::new("git")
                    .args(["config", "--file"])
                    .arg(&config)
                    .arg(setting)
                    .arg("https://fixture.invalid/repo")
                    .status()
                    .unwrap()
                    .success()
            );
            let result = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "registry::tests::clone_errors_do_not_expose_direct_or_rewritten_secrets",
                    "--nocapture",
                ])
                .env(CHILD, &root)
                .env("HOME", &root)
                .env("GIT_CONFIG_GLOBAL", &config)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_TERMINAL_PROMPT", "0")
                .env("NO_PROXY", "*")
                .env_remove("GIT_CONFIG_COUNT")
                .env_remove("GIT_CONFIG_PARAMETERS")
                .env_remove("GIT_ALLOW_PROTOCOL")
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            return;
        }
        let root = PathBuf::from(std::env::var_os(CHILD).unwrap());
        let raw = Command::new("git")
            .args(["clone", "--", "https://fixture.invalid/repo"])
            .arg(root.join("raw-control"))
            .output()
            .unwrap();
        assert!(!raw.status.success());
        assert!(
            String::from_utf8_lossy(&raw.stderr).contains(SECRET),
            "control did not produce a sensitive diagnostic"
        );
        let error = clone_repository("https://fixture.invalid/repo", &root.join("rewritten"))
            .await
            .err()
            .expect("rewritten clone must fail");
        assert!(
            error
                .to_string()
                .contains("remote repository could not be accessed")
        );
        assert!(!format!("{error:?}").contains(SECRET));

        // Direct URL userinfo is independently covered using a closed local
        // port; no external network or real credential is involved.
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let url = format!("http://user:{SECRET}@127.0.0.1:{port}/repo");
        let error = tokio::time::timeout(
            Duration::from_secs(8),
            clone_repository(&url, &root.join("direct")),
        )
        .await
        .unwrap()
        .err()
        .expect("unreachable clone must fail");
        assert!(!format!("{error:?}").contains(SECRET));
        assert!(!error.to_string().contains("user:"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn clone_rejects_rewritten_external_helpers_even_with_allowlist() {
        use std::{os::unix::fs::PermissionsExt, process::Command};
        const CHILD: &str = "DESKTOP_TEST_CLONE_PROTOCOL_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let fixture = RegistryFixture::new();
            let root = fixture.directory.clone();
            let script = root.join("external-helper");
            fs::write(
                &script,
                "#!/bin/sh\n: > \"$DESKTOP_CLONE_TEST_MARKER\"\nexit 1\n",
            )
            .unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
            let config = root.join("gitconfig");
            let setting = format!("url.ext::{}.insteadOf", script.display());
            assert!(
                Command::new("git")
                    .args(["config", "--file"])
                    .arg(&config)
                    .arg(setting)
                    .arg("https://unsafe.invalid/repo")
                    .status()
                    .unwrap()
                    .success()
            );
            let result = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "registry::tests::clone_rejects_rewritten_external_helpers_even_with_allowlist",
                    "--nocapture",
                ])
                .env(CHILD, &root)
                .env("HOME", &root)
                .env("GIT_CONFIG_GLOBAL", &config)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_ALLOW_PROTOCOL", "file:ext:ext.exe")
                .env("DESKTOP_CLONE_TEST_MARKER", root.join("executed"))
                .env_remove("GIT_CONFIG_COUNT")
                .env_remove("GIT_CONFIG_PARAMETERS")
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert!(!root.join("executed").exists());
            return;
        }
        let root = PathBuf::from(std::env::var_os(CHILD).unwrap());
        let source = root.join("source.git");
        assert!(
            Command::new("git")
                .args(["init", "--bare", "-q"])
                .arg(&source)
                .status()
                .unwrap()
                .success()
        );
        // Prove that this isolated rewrite really invokes only our fixture
        // helper without the guard; otherwise an unused config would make the
        // negative assertion below vacuous.
        let control = Command::new("git")
            .args(["clone", "--", "https://unsafe.invalid/repo"])
            .arg(root.join("control"))
            .output()
            .unwrap();
        assert!(!control.status.success());
        assert!(root.join("executed").exists());
        fs::remove_file(root.join("executed")).unwrap();
        assert!(
            clone_repository("https://unsafe.invalid/repo", &root.join("blocked"))
                .await
                .is_err()
        );
        assert!(!root.join("executed").exists());
        assert!(
            clone_repository("ext.exe::invalid", &root.join("blocked-exe"))
                .await
                .is_err()
        );
        let local = format!("file://{}", source.display());
        assert!(
            clone_repository(&local, &root.join("allowed"))
                .await
                .is_ok()
        );
    }

    struct RegistryFixture {
        directory: PathBuf,
        registry: Registry,
    }
    impl RegistryFixture {
        fn new() -> Self {
            let directory =
                std::env::temp_dir().join(format!("desktop-registry-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&directory).unwrap();
            Self {
                registry: Registry::new(directory.join("registry.json")),
                directory,
            }
        }
    }
    impl Drop for RegistryFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }
    fn disable_account(
        registry: &Registry,
        record: &SavedRepositoryRecord,
    ) -> SavedRepositoryRecord {
        registry
            .compare_and_replace_account(
                &record.id,
                record.account.revision,
                StoredAccount {
                    revision: record.account.revision + 1,
                    assignment: crate::accounts::AccountAssignment::None {},
                    association: None,
                    source_remote: record.account.source_remote.clone(),
                },
            )
            .unwrap()
    }

    #[test]
    fn moving_worktree_records_preserves_identity_and_other_selection() {
        let fixture = RegistryFixture::new();
        let source = fixture.directory.join("source");
        let target = fixture.directory.join("target");
        let other = fixture.directory.join("other");
        fixture.registry.add(source.clone(), true).unwrap();
        let record = fixture.registry.record_for_path(&source).unwrap().unwrap();
        let record = disable_account(&fixture.registry, &record);
        fixture.registry.add(other.clone(), true).unwrap();
        let moved = fixture
            .registry
            .relocate_worktree_record(&record, target.clone(), true)
            .unwrap()
            .unwrap();
        assert_eq!(moved.id, record.id);
        assert_eq!(moved.account.revision, record.account.revision);
        assert_eq!(moved.account.assignment, crate::AccountAssignment::None {});
        let state = fixture.registry.read().unwrap();
        assert_eq!(state.selected, Some(other));
        assert!(!state.repositories.contains(&source));
        assert!(state.repositories.contains(&target));
    }

    #[test]
    fn clone_journal_does_not_change_the_legacy_registry_schema() {
        #[allow(dead_code)]
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct LegacyRegistry {
            version: u32,
            selected: Option<serde_json::Value>,
            repositories: Vec<serde_json::Value>,
            #[serde(default)]
            records: Vec<serde_json::Value>,
            #[serde(default)]
            publication_intents: Vec<serde_json::Value>,
        }
        let fixture = RegistryFixture::new();
        let root = fixture.directory.join("repo");
        fixture.registry.add(root, true).unwrap();
        let intent = CloneIntent {
            id: uuid::Uuid::new_v4().to_string(),
            phase: "downloading".into(),
            url: "https://github.com/alice/repository.git".into(),
            certificate: None,
            stage: fixture
                .directory
                .join(format!(".desktop-clone-{}", uuid::Uuid::new_v4()))
                .join("repository"),
            destination: fixture.directory.join("clone"),
            device: 1,
            inode: 1,
            parent_identity: Some((1, 1)),
            account: StoredAccount {
                revision: 1,
                assignment: crate::AccountAssignment::Assigned {
                    host: "github.com".into(),
                    login: "alice".into(),
                    id: 1,
                },
                association: None,
                source_remote: Some("origin".into()),
            },
        };
        fixture.registry.begin_clone_intent(intent).unwrap();
        assert!(
            !fixture.registry.clone_intents_path().exists(),
            "new intents must be invisible to older binaries"
        );
        assert!(fixture.registry.clone_intents_v2_path().exists());
        let bytes = fs::read(&fixture.registry.path).unwrap();
        let legacy: LegacyRegistry = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(legacy.version, 1);
        assert_eq!(fixture.registry.read().unwrap().clone_intents.len(), 1);
        let public = fixture.registry.read().unwrap().public_view().unwrap();
        assert!(public.get("clone_intents").is_none());
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[tokio::test]
    async fn legacy_clone_journal_remains_readable_and_inspection_only() {
        let fixture = RegistryFixture::new();
        let intent = CloneIntent {
            id: uuid::Uuid::new_v4().to_string(),
            phase: "ready".into(),
            url: "https://github.com/alice/repository.git".into(),
            certificate: Some("0".repeat(64)),
            stage: fixture
                .directory
                .join(format!(".desktop-clone-{}", uuid::Uuid::new_v4()))
                .join("repository"),
            destination: fixture.directory.join("old"),
            device: 1,
            inode: 1,
            parent_identity: None,
            account: StoredAccount {
                revision: 1,
                assignment: crate::AccountAssignment::Assigned {
                    host: "github.com".into(),
                    login: "alice".into(),
                    id: 1,
                },
                association: None,
                source_remote: Some("origin".into()),
            },
        };
        let legacy = serde_json::to_vec(&vec![intent.clone()]).unwrap();
        fs::write(fixture.registry.clone_intents_path(), &legacy).unwrap();
        let mut next = intent.clone();
        next.id = uuid::Uuid::new_v4().to_string();
        next.stage = fixture
            .directory
            .join(format!(".desktop-clone-{}", uuid::Uuid::new_v4()))
            .join("repository");
        next.destination = fixture.directory.join("new");
        next.phase = "downloading".into();
        next.certificate = None;
        next.parent_identity = Some((1, 1));
        fixture.registry.begin_clone_intent(next).unwrap();
        assert_eq!(fixture.registry.read().unwrap().clone_intents.len(), 2);
        let records = fixture.registry.clone_recovery_records().unwrap();
        assert!(
            records
                .as_array()
                .unwrap()
                .iter()
                .all(|record| record["recoverable"] == false)
        );
        for record in records.as_array().unwrap() {
            let keys = record
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(
                keys,
                [
                    "id",
                    "phase",
                    "stage",
                    "destination",
                    "recoverable",
                    "conflicted"
                ]
                .into_iter()
                .collect()
            );
        }
        assert_eq!(
            fs::read(fixture.registry.clone_intents_path()).unwrap(),
            legacy
        );
        let error =
            recover_selected_clone_unavailable_implementation(fixture.registry.clone(), &intent.id)
                .await
                .err()
                .expect("legacy intent must not be recovered");
        assert!(error.to_string().contains("manual inspection"));
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[tokio::test]
    async fn conflicting_legacy_and_v2_clones_leave_registry_readable() {
        let fixture = RegistryFixture::new();
        let unrelated = fixture.directory.join("saved");
        fixture.registry.add(unrelated.clone(), true).unwrap();
        let mut intent = CloneIntent {
            id: uuid::Uuid::new_v4().to_string(),
            phase: "ready".into(),
            url: "https://github.com/alice/repository.git".into(),
            certificate: Some("0".repeat(64)),
            stage: fixture
                .directory
                .join(format!(".desktop-clone-{}", uuid::Uuid::new_v4()))
                .join("repository"),
            destination: fixture.directory.join("target"),
            device: 1,
            inode: 1,
            parent_identity: Some((1, 1)),
            account: StoredAccount {
                revision: 1,
                assignment: crate::AccountAssignment::Assigned {
                    host: "github.com".into(),
                    login: "alice".into(),
                    id: 1,
                },
                association: None,
                source_remote: Some("origin".into()),
            },
        };
        fixture.registry.begin_clone_intent(intent.clone()).unwrap();
        intent.id = uuid::Uuid::new_v4().to_string();
        intent.stage = fixture
            .directory
            .join(format!(".desktop-clone-{}", uuid::Uuid::new_v4()))
            .join("repository");
        intent.parent_identity = None;
        fs::write(
            fixture.registry.clone_intents_path(),
            serde_json::to_vec(&vec![intent.clone()]).unwrap(),
        )
        .unwrap();
        let state = fixture.registry.read().unwrap();
        assert_eq!(state.selected, Some(unrelated));
        assert_eq!(state.clone_intents.len(), 2);
        let records = fixture.registry.clone_recovery_records().unwrap();
        assert!(
            records
                .as_array()
                .unwrap()
                .iter()
                .all(|record| record["conflicted"] == true && record["recoverable"] == false)
        );
        assert!(fixture.registry.ensure_uncontested_clone(&intent).is_err());
        assert!(
            recover_selected_clone_unavailable_implementation(fixture.registry.clone(), &intent.id)
                .await
                .is_err()
        );
        assert!(!intent.destination.exists());
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn independently_full_legacy_and_v2_journals_keep_registry_readable() {
        let fixture = RegistryFixture::new();
        let unrelated = fixture.directory.join("saved");
        fixture.registry.add(unrelated.clone(), true).unwrap();
        let mut legacy = Vec::new();
        let mut current = Vec::new();
        for index in 0..2000_u128 {
            let id = uuid::Uuid::from_u128(index + 1).to_string();
            let intent = CloneIntent {
                id: id.clone(),
                phase: "downloading".into(),
                url: "https://github.com/alice/repository.git".into(),
                certificate: None,
                stage: fixture
                    .directory
                    .join(format!(".desktop-clone-{id}"))
                    .join("repository"),
                destination: fixture.directory.join(format!("target-{index}")),
                device: 1,
                inode: 1,
                parent_identity: (index >= 1000).then_some((1, 1)),
                account: StoredAccount {
                    revision: 1,
                    assignment: crate::AccountAssignment::Assigned {
                        host: "github.com".into(),
                        login: "alice".into(),
                        id: 1,
                    },
                    association: None,
                    source_remote: Some("origin".into()),
                },
            };
            if index < 1000 {
                legacy.push(intent);
            } else {
                current.push(intent);
            }
        }
        fs::write(
            fixture.registry.clone_intents_path(),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        fs::write(
            fixture.registry.clone_intents_v2_path(),
            serde_json::to_vec(&current).unwrap(),
        )
        .unwrap();
        let state = fixture.registry.read().unwrap();
        assert_eq!(state.selected, Some(unrelated));
        assert_eq!(state.clone_intents.len(), 2000);
        fs::remove_file(fixture.registry.clone_intents_path()).unwrap();
        let mut extra = current[0].clone();
        extra.id = uuid::Uuid::new_v4().to_string();
        extra.stage = fixture
            .directory
            .join(format!(".desktop-clone-{}", uuid::Uuid::new_v4()))
            .join("repository");
        extra.destination = fixture.directory.join("unique-over-limit");
        assert!(fixture.registry.begin_clone_intent(extra).is_err());
        assert_eq!(fixture.registry.read().unwrap().clone_intents.len(), 1000);
        let mut oversized = current[0].clone();
        oversized.url = format!("https://github.com/alice/{}", "a".repeat(16 * 1024 * 1024));
        assert!(
            fixture
                .registry
                .write_clone_intents(&current, &[oversized])
                .is_err()
        );
        assert_eq!(fixture.registry.read().unwrap().clone_intents.len(), 1000);
    }

    #[cfg(unix)]
    #[test]
    fn private_sidecar_reads_are_bounded_and_do_not_follow_replaced_paths() {
        use std::os::unix::fs::symlink;
        let fixture = RegistryFixture::new();
        let path = fixture.directory.join("sidecar");
        let victim = fixture.directory.join("victim");
        fs::write(&path, b"1\n").unwrap();
        fs::write(&victim, vec![b'x'; 1024]).unwrap();
        let pinned = open_private_sidecar(&path).unwrap().unwrap();
        fs::rename(&path, fixture.directory.join("original")).unwrap();
        symlink(&victim, &path).unwrap();
        assert_eq!(read_private_descriptor(&pinned, 32).unwrap(), b"1\n");
        assert!(read_private_sidecar(&path, 32).is_err());
        fs::remove_file(&path).unwrap();
        fs::write(&path, vec![b'1'; 33]).unwrap();
        assert!(read_private_sidecar(&path, 32).is_err());
        assert_eq!(fs::metadata(victim).unwrap().len(), 1024);
    }

    #[test]
    fn moving_worktree_records_checks_selection_revision_and_collision() {
        let fixture = RegistryFixture::new();
        let source = fixture.directory.join("source");
        let target = fixture.directory.join("target");
        fixture.registry.add(source.clone(), true).unwrap();
        let stale = fixture.registry.record_for_path(&source).unwrap().unwrap();
        let current = disable_account(&fixture.registry, &stale);
        assert!(
            fixture
                .registry
                .relocate_worktree_record(&stale, target.clone(), true)
                .is_err()
        );
        fixture.registry.add(target.clone(), false).unwrap();
        assert!(
            fixture
                .registry
                .relocate_worktree_record(&current, target.clone(), true)
                .is_err()
        );
        fixture.registry.forget(&target).unwrap();
        fixture
            .registry
            .relocate_worktree_record(&current, target.clone(), true)
            .unwrap()
            .unwrap();
        assert_eq!(
            fixture.registry.read().unwrap().selected,
            Some(target.clone())
        );
        let moved = fixture.registry.record_for_path(&target).unwrap().unwrap();
        fixture
            .registry
            .relocate_worktree_record(&moved, source.clone(), false)
            .unwrap()
            .unwrap();
        assert_eq!(fixture.registry.read().unwrap().selected, None);
        fixture.registry.forget(&source).unwrap();
        assert!(
            fixture
                .registry
                .relocate_worktree_record(&moved, target, true)
                .unwrap()
                .is_none()
        );
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn failed_clone_admission_preserves_empty_replacement_and_original_stage() {
        let fixture = RegistryFixture::new();
        let container = fixture
            .directory
            .join(format!(".desktop-clone-{}", uuid::Uuid::new_v4()));
        let stage = container.join("repository");
        let original = container.join("original");
        fs::create_dir_all(&stage).unwrap();
        fs::rename(&stage, &original).unwrap();
        fs::create_dir(&stage).unwrap();
        let invalid_registry = Registry::new(fixture.directory.clone());
        let intent = CloneIntent {
            id: uuid::Uuid::new_v4().to_string(),
            phase: "downloading".into(),
            url: "https://github.com/alice/repository.git".into(),
            certificate: None,
            stage: stage.clone(),
            destination: fixture.directory.join("target"),
            device: 1,
            inode: 1,
            parent_identity: Some((1, 1)),
            account: StoredAccount {
                revision: 1,
                assignment: crate::AccountAssignment::Assigned {
                    host: "github.com".into(),
                    login: "alice".into(),
                    id: 1,
                },
                association: None,
                source_remote: Some("origin".into()),
            },
        };
        assert!(invalid_registry.begin_clone_intent(intent).is_err());
        assert!(stage.is_dir());
        assert!(original.is_dir());
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn clone_descriptor_path_fails_closed_when_procfs_is_unavailable() {
        let fixture = RegistryFixture::new();
        let directory = fs::File::open(&fixture.directory).unwrap();
        assert!(
            pinned_clone_directory(&directory, &fixture.directory.join("missing-proc")).is_err()
        );
        if Path::new("/proc").exists() {
            let pinned = pinned_clone_directory(&directory, Path::new("/proc")).unwrap();
            assert!(fs::metadata(pinned).unwrap().is_dir());
        }
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn clone_parent_walk_rejects_swapped_symlink_without_creating_children() {
        use std::os::unix::fs::symlink;
        let fixture = RegistryFixture::new();
        let approved = fixture.directory.join("approved");
        let protected = fixture.directory.join("protected");
        let missing = fixture.directory.join("missing").join("nested");
        assert!(open_clone_parent(&missing).is_err());
        assert!(canonical_clone_destination(&missing.join("repo")).is_err());
        assert!(!fixture.directory.join("missing").exists());
        fs::create_dir(&approved).unwrap();
        fs::create_dir(&protected).unwrap();
        let destination = canonical_clone_destination(&approved.join("repo")).unwrap();
        fs::rename(&approved, fixture.directory.join("displaced")).unwrap();
        symlink(&protected, &approved).unwrap();
        assert!(open_clone_parent(destination.parent().unwrap()).is_err());
        assert!(!protected.join("repo").exists());
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn clone_destination_resolves_symlink_parent_before_dot_dot() {
        use std::os::unix::fs::symlink;
        let fixture = RegistryFixture::new();
        let real = fixture.directory.join("real");
        fs::create_dir_all(real.join("nested")).unwrap();
        let alias = fixture.directory.join("alias");
        symlink(real.join("nested"), &alias).unwrap();
        assert_eq!(
            canonical_clone_destination(&alias.join("..").join("repo")).unwrap(),
            real.join("repo")
        );
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[tokio::test]
    async fn clone_discovery_stays_on_pinned_directory_after_path_swap() {
        use std::os::unix::fs::symlink;
        let fixture = RegistryFixture::new();
        let stage = fixture.directory.join("stage");
        let displaced = fixture.directory.join("displaced");
        let victim = fixture.directory.join("victim");
        fs::create_dir(&stage).unwrap();
        fs::create_dir(&victim).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "--quiet"])
                .arg(&stage)
                .status()
                .unwrap()
                .success()
        );
        let handle = fs::File::open(&stage).unwrap();
        let pinned = pinned_clone_directory(&handle, Path::new("/proc")).unwrap();
        fs::rename(&stage, &displaced).unwrap();
        symlink(&victim, &stage).unwrap();
        let repository = Repository::open_pinned(&pinned).await.unwrap();
        assert_eq!(repository.path(), displaced);
        assert_eq!(fs::read_dir(victim).unwrap().count(), 0);
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn clone_sensitive_home_rejects_symlinked_credential_directory() {
        use std::os::unix::fs::symlink;
        let fixture = RegistryFixture::new();
        let home = fixture.directory.join("home");
        let credentials = fixture.directory.join("credentials");
        fs::create_dir(&home).unwrap();
        fs::create_dir(&credentials).unwrap();
        symlink(&credentials, home.join(".ssh")).unwrap();
        assert!(
            ensure_clone_destination_outside_sensitive_home(&credentials.join("new"), &home)
                .is_err()
        );
        assert!(
            ensure_clone_destination_outside_sensitive_home(&home.join("projects/repo"), &home)
                .is_ok()
        );
        assert_eq!(fs::read_dir(credentials).unwrap().count(), 0);
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn clone_recovery_sensitive_policy_uses_current_home() {
        let fixture = RegistryFixture::new();
        let old_home = fixture.directory.join("old-home");
        let current_home = fixture.directory.join("current-home");
        fs::create_dir_all(current_home.join(".ssh")).unwrap();
        fs::create_dir(&old_home).unwrap();
        let destination = current_home.join(".ssh").join("repo");
        assert!(ensure_clone_destination_outside_sensitive_home(&destination, &old_home).is_ok());
        assert!(
            ensure_clone_destination_outside_sensitive_home(&destination, &current_home).is_err()
        );
        assert!(!destination.exists());
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn clone_parent_moved_into_sensitive_home_fails_before_staging() {
        let fixture = RegistryFixture::new();
        let home = fixture.directory.join("home");
        let protected = home.join(".ssh");
        let approved = home.join("projects");
        fs::create_dir_all(&protected).unwrap();
        fs::create_dir(&approved).unwrap();
        let handle = open_clone_parent(&approved).unwrap();
        let moved = protected.join("moved");
        fs::rename(&approved, &moved).unwrap();
        assert!(revalidate_clone_parent(&handle, &approved, &home).is_err());
        assert_eq!(fs::read_dir(&moved).unwrap().count(), 0);
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn detached_clone_registry_write_owns_its_directory_descriptors() {
        let fixture = RegistryFixture::new();
        let owned = fixture.directory.join("owned");
        let stage = owned.join("repository");
        let unrelated = fixture.directory.join("unrelated");
        fs::create_dir_all(&stage).unwrap();
        fs::create_dir(&unrelated).unwrap();
        let container_handle = std::sync::Arc::new(fs::File::open(&owned).unwrap());
        let stage_handle = std::sync::Arc::new(fs::File::open(&stage).unwrap());
        let registry = Registry::new(
            pinned_clone_directory(&container_handle, Path::new("/proc"))
                .unwrap()
                .join("registry.json"),
        )
        .with_owned_handles(vec![container_handle.clone(), stage_handle.clone()]);
        let stage_path = pinned_clone_directory(&stage_handle, Path::new("/proc")).unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let work = tokio::spawn(async move {
            crate::cancellation::blocking(move || {
                started_tx.send(()).unwrap();
                resume_rx.recv().unwrap();
                let account = StoredAccount {
                    revision: 1,
                    assignment: crate::AccountAssignment::Assigned {
                        host: "github.com".into(),
                        login: "alice".into(),
                        id: 1,
                    },
                    association: None,
                    source_remote: Some("origin".into()),
                };
                let result = registry.add_assigned_clone(stage_path, account);
                done_tx.send(result.is_ok()).unwrap();
                result.map(|_| ())
            })
            .await
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        work.abort();
        drop(container_handle);
        drop(stage_handle);
        let _replacement = fs::File::open(&unrelated).unwrap();
        resume_tx.send(()).unwrap();
        assert!(
            done_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
        );
        assert!(owned.join("registry.json").exists());
        assert_eq!(fs::read_dir(unrelated).unwrap().count(), 0);
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn clone_publication_is_no_replace_and_recoverable_after_rename() {
        use std::os::unix::fs::MetadataExt;
        let fixture = RegistryFixture::new();
        let container = fixture
            .directory
            .join(format!(".desktop-clone-{}", uuid::Uuid::new_v4()));
        let stage = container.join("repository");
        fs::create_dir_all(&stage).unwrap();
        fs::write(stage.join("data"), "owned").unwrap();
        let destination = fixture.directory.join("target");
        let stat = fs::symlink_metadata(&stage).unwrap();
        let mut intent = CloneIntent {
            id: uuid::Uuid::new_v4().to_string(),
            phase: "downloading".into(),
            url: "https://github.com/alice/repository.git".into(),
            certificate: None,
            stage: stage.clone(),
            destination: destination.clone(),
            device: stat.dev(),
            inode: stat.ino(),
            parent_identity: Some((
                fs::metadata(&fixture.directory).unwrap().dev(),
                fs::metadata(&fixture.directory).unwrap().ino(),
            )),
            account: StoredAccount {
                revision: 1,
                assignment: crate::AccountAssignment::Assigned {
                    host: "github.com".into(),
                    login: "alice".into(),
                    id: 1,
                },
                association: None,
                source_remote: Some("origin".into()),
            },
        };
        fixture.registry.begin_clone_intent(intent.clone()).unwrap();
        assert!(
            fixture
                .registry
                .advance_clone_intent(&intent, "checkout", "ready", None)
                .is_err()
        );
        fixture
            .registry
            .advance_clone_intent(&intent, "downloading", "checkout", None)
            .unwrap();
        let certificate = clone_certificate(&stage).unwrap();
        fixture
            .registry
            .advance_clone_intent(&intent, "checkout", "ready", Some(certificate.clone()))
            .unwrap();
        intent.phase = "ready".into();
        intent.certificate = Some(certificate);
        assert_eq!(
            fixture.registry.clone_recovery_records().unwrap()[0]["recoverable"],
            true
        );
        assert!(
            fixture
                .registry
                .read()
                .unwrap()
                .public_view()
                .unwrap()
                .get("clone_intents")
                .is_none()
        );
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("data"), "unrelated").unwrap();
        assert!(publish_clone_intent(&fixture.registry, &intent).is_err());
        assert_eq!(
            fs::read_to_string(destination.join("data")).unwrap(),
            "unrelated"
        );
        assert_eq!(fs::read_to_string(stage.join("data")).unwrap(), "owned");
        fs::remove_dir_all(&destination).unwrap();
        publish_clone_intent(&fixture.registry, &intent).unwrap();
        assert!(!stage.exists());
        // A crash after rename but before registry update is safe to resume.
        publish_clone_intent(&fixture.registry, &intent).unwrap();
        let moved = fixture.directory.join("moved");
        fs::rename(&destination, &moved).unwrap();
        fs::create_dir(&destination).unwrap();
        assert!(fixture.registry.finish_clone_intent(&intent, true).is_err());
        fs::remove_dir(&destination).unwrap();
        fs::rename(&moved, &destination).unwrap();
        // Simulate a crash after the main registry was written but before the
        // separate recovery journal was cleared.
        fixture
            .registry
            .update(|data| {
                data.repositories.push(destination.clone());
                data.persist_record(SavedRepositoryRecord {
                    id: intent.id.clone(),
                    path: destination.clone(),
                    account: intent.account.clone(),
                });
                Ok(())
            })
            .unwrap();
        assert_eq!(fixture.registry.read().unwrap().clone_intents.len(), 1);
        let record = fixture.registry.finish_clone_intent(&intent, true).unwrap();
        assert_eq!(record.account.assignment, intent.account.assignment);
        let state = fixture.registry.read().unwrap();
        assert_eq!(state.selected, Some(destination));
        assert!(state.clone_intents.is_empty());
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn clone_publication_rejects_same_inode_content_changes() {
        use std::os::unix::fs::MetadataExt;
        let fixture = RegistryFixture::new();
        let stage = fixture
            .directory
            .join(format!(".desktop-clone-{}", uuid::Uuid::new_v4()))
            .join("repository");
        fs::create_dir_all(&stage).unwrap();
        fs::write(stage.join("data"), "owned").unwrap();
        let stat = fs::symlink_metadata(&stage).unwrap();
        let intent = CloneIntent {
            id: uuid::Uuid::new_v4().to_string(),
            phase: "ready".into(),
            url: "https://github.com/alice/repository.git".into(),
            certificate: Some(clone_certificate(&stage).unwrap()),
            stage: stage.clone(),
            destination: fixture.directory.join("target"),
            device: stat.dev(),
            inode: stat.ino(),
            parent_identity: Some((
                fs::metadata(&fixture.directory).unwrap().dev(),
                fs::metadata(&fixture.directory).unwrap().ino(),
            )),
            account: StoredAccount {
                revision: 1,
                assignment: crate::AccountAssignment::Assigned {
                    host: "github.com".into(),
                    login: "alice".into(),
                    id: 1,
                },
                association: None,
                source_remote: Some("origin".into()),
            },
        };
        fixture.registry.begin_clone_intent(intent.clone()).unwrap();
        fs::write(stage.join("data"), "changed").unwrap();
        assert_eq!(fs::symlink_metadata(&stage).unwrap().ino(), intent.inode);
        assert!(publish_clone_intent(&fixture.registry, &intent).is_err());
        assert!(!intent.destination.exists());
        assert_eq!(fs::read_to_string(stage.join("data")).unwrap(), "changed");
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn clone_publication_rejects_parent_replacement() {
        use std::os::unix::fs::{MetadataExt, symlink};
        let fixture = RegistryFixture::new();
        let approved = fixture.directory.join("approved");
        let protected = fixture.directory.join("protected");
        fs::create_dir(&approved).unwrap();
        fs::create_dir(&protected).unwrap();
        let stage = approved
            .join(format!(".desktop-clone-{}", uuid::Uuid::new_v4()))
            .join("repository");
        fs::create_dir_all(&stage).unwrap();
        fs::write(stage.join("data"), "owned").unwrap();
        let metadata = fs::metadata(&stage).unwrap();
        let parent = fs::metadata(&approved).unwrap();
        let intent = CloneIntent {
            id: uuid::Uuid::new_v4().to_string(),
            phase: "ready".into(),
            url: "https://github.com/alice/repository.git".into(),
            certificate: Some(clone_certificate(&stage).unwrap()),
            stage,
            destination: approved.join("target"),
            device: metadata.dev(),
            inode: metadata.ino(),
            parent_identity: Some((parent.dev(), parent.ino())),
            account: StoredAccount {
                revision: 1,
                assignment: crate::AccountAssignment::Assigned {
                    host: "github.com".into(),
                    login: "alice".into(),
                    id: 1,
                },
                association: None,
                source_remote: Some("origin".into()),
            },
        };
        fs::rename(&approved, fixture.directory.join("displaced")).unwrap();
        symlink(&protected, &approved).unwrap();
        assert!(publish_clone_intent(&fixture.registry, &intent).is_err());
        assert_eq!(fs::read_dir(&protected).unwrap().count(), 0);
        fs::remove_file(&approved).unwrap();
        fs::create_dir(&approved).unwrap();
        assert!(publish_clone_intent(&fixture.registry, &intent).is_err());
        assert_eq!(fs::read_dir(&approved).unwrap().count(), 0);
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[tokio::test]
    async fn clone_source_swap_does_not_register_or_overwrite_user_data() {
        use std::os::unix::fs::MetadataExt;
        let fixture = RegistryFixture::new();
        let stage = fixture
            .directory
            .join(format!(".desktop-clone-{}", uuid::Uuid::new_v4()))
            .join("repository");
        fs::create_dir_all(&stage).unwrap();
        fs::write(stage.join("reviewed"), "preserve").unwrap();
        let destination = fixture.directory.join("target");
        let moved = stage.with_file_name("original");
        let metadata = fs::metadata(&stage).unwrap();
        let parent = fs::metadata(&fixture.directory).unwrap();
        let intent = CloneIntent {
            id: uuid::Uuid::new_v4().to_string(),
            phase: "ready".into(),
            url: "https://github.com/alice/repository.git".into(),
            certificate: Some(clone_certificate(&stage).unwrap()),
            stage: stage.clone(),
            destination: destination.clone(),
            device: metadata.dev(),
            inode: metadata.ino(),
            parent_identity: Some((parent.dev(), parent.ino())),
            account: StoredAccount {
                revision: 1,
                assignment: crate::AccountAssignment::Assigned {
                    host: "github.com".into(),
                    login: "alice".into(),
                    id: 1,
                },
                association: None,
                source_remote: Some("origin".into()),
            },
        };
        fixture.registry.begin_clone_intent(intent.clone()).unwrap();
        assert!(
            publish_clone_intent_before_rename(
                &fixture.registry,
                &intent,
                || Ok(()),
                || {
                    fs::rename(&stage, &moved)?;
                    fs::create_dir(&stage)?;
                    fs::write(stage.join("replacement"), "preserve")?;
                    Ok(())
                }
            )
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(moved.join("reviewed")).unwrap(),
            "preserve"
        );
        assert_eq!(
            fs::read_to_string(destination.join("replacement")).unwrap(),
            "preserve"
        );
        assert!(
            fixture
                .registry
                .read()
                .unwrap()
                .record_for_path(&destination)
                .is_none()
        );
        assert!(
            recover_selected_clone_unavailable_implementation(fixture.registry.clone(), &intent.id)
                .await
                .is_err()
        );
        assert_eq!(
            fs::read_to_string(moved.join("reviewed")).unwrap(),
            "preserve"
        );
        assert_eq!(
            fs::read_to_string(destination.join("replacement")).unwrap(),
            "preserve"
        );
        assert!(
            fixture
                .registry
                .read()
                .unwrap()
                .record_for_path(&destination)
                .is_none()
        );
        assert_eq!(fixture.registry.read().unwrap().clone_intents.len(), 1);
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn legacy_intent_arriving_before_publish_blocks_rename() {
        use std::os::unix::fs::MetadataExt;
        let fixture = RegistryFixture::new();
        let stage = fixture
            .directory
            .join(format!(".desktop-clone-{}", uuid::Uuid::new_v4()))
            .join("repository");
        fs::create_dir_all(&stage).unwrap();
        fs::write(stage.join("data"), "preserve").unwrap();
        let metadata = fs::metadata(&stage).unwrap();
        let parent = fs::metadata(&fixture.directory).unwrap();
        let intent = CloneIntent {
            id: uuid::Uuid::new_v4().to_string(),
            phase: "ready".into(),
            url: "https://github.com/alice/repository.git".into(),
            certificate: Some(clone_certificate(&stage).unwrap()),
            stage: stage.clone(),
            destination: fixture.directory.join("target"),
            device: metadata.dev(),
            inode: metadata.ino(),
            parent_identity: Some((parent.dev(), parent.ino())),
            account: StoredAccount {
                revision: 1,
                assignment: crate::AccountAssignment::Assigned {
                    host: "github.com".into(),
                    login: "alice".into(),
                    id: 1,
                },
                association: None,
                source_remote: Some("origin".into()),
            },
        };
        fixture.registry.begin_clone_intent(intent.clone()).unwrap();
        let mut legacy = intent.clone();
        legacy.id = uuid::Uuid::new_v4().to_string();
        legacy.stage = fixture
            .directory
            .join(format!(".desktop-clone-{}", uuid::Uuid::new_v4()))
            .join("repository");
        legacy.parent_identity = None;
        assert!(
            publish_clone_intent_before_rename(
                &fixture.registry,
                &intent,
                || {
                    fs::write(
                        fixture.registry.clone_intents_path(),
                        serde_json::to_vec(&vec![legacy])?,
                    )?;
                    Ok(())
                },
                || Ok(())
            )
            .is_err()
        );
        assert!(!intent.destination.exists());
        assert_eq!(fs::read_to_string(stage.join("data")).unwrap(), "preserve");
        assert_eq!(fixture.registry.read().unwrap().clone_intents.len(), 2);
    }

    #[test]
    fn publication_identity_requires_matching_actor_and_reviewed_source() {
        let mut identity = PublicationIdentity {
            host: "github.com".into(),
            owner: "Alice".into(),
            name: "project".into(),
            actor_login: "Alice".into(),
            actor_id: 42,
            owner_id: 42,
            target: crate::AccountTarget::Assigned {
                host: "github.com".into(),
                login: "Alice".into(),
            },
            account_basis: "a".repeat(64),
            source_fingerprint: "b".repeat(64),
        };
        assert!(identity.validate().is_ok());
        let hash = identity.destination_hash().unwrap();
        identity.owner = "ALICE".into();
        assert_eq!(identity.destination_hash().unwrap(), hash);
        identity.owner_id = 43;
        assert!(identity.validate().is_err());
        identity.owner_id = 42;
        identity.target = crate::AccountTarget::None {};
        assert!(identity.validate().is_err());
        identity.target = crate::AccountTarget::Effective {};
        identity.source_fingerprint = "invalid".into();
        assert!(identity.validate().is_err());
    }

    #[test]
    fn publication_ledger_is_persistent_but_omitted_from_public_views() {
        let fixture = RegistryFixture::new();
        let path = fixture.directory.join("repo");
        fixture.registry.add(path.clone(), true).unwrap();
        let record = fixture.registry.record_for_path(&path).unwrap().unwrap();
        let mut data = fixture.registry.read().unwrap();
        data.publication_intents.push(PublicationIntent {
            id: uuid::Uuid::new_v4().to_string(),
            repository_id: "a".repeat(64),
            common_id: "b".repeat(64),
            record_id: record.id,
            phase: "created".into(),
            destination_hash: "c".repeat(64),
            remote_id: Some(8_765_432_109_876_543),
            acknowledged: false,
            identity: None,
        });
        fs::write(&fixture.registry.path, serde_json::to_vec(&data).unwrap()).unwrap();
        let loaded = fixture.registry.read().unwrap();
        assert_eq!(loaded.publication_intents.len(), 1);
        let persisted = serde_json::to_value(&loaded).unwrap();
        let view = loaded.public_view().unwrap();
        assert!(persisted.get("publication_intents").is_some());
        assert!(view.get("publication_intents").is_none());
        for key in ["version", "selected", "repositories", "records"] {
            assert_eq!(view.get(key), persisted.get(key));
        }
        assert!(!view.to_string().contains("8765432109876543"));
        fixture.registry.forget(&path).unwrap();
        let retained = fixture.registry.read().unwrap();
        assert_eq!(retained.publication_intents.len(), 1);
        assert!(
            retained
                .public_view()
                .unwrap()
                .get("publication_intents")
                .is_none()
        );
    }

    #[test]
    fn legacy_record_lookup_is_read_only_and_account_upgrade_keeps_identity() {
        let fixture = RegistryFixture::new();
        let path = fixture.directory.join("repo");
        let original = serde_json::to_vec(
            &serde_json::json!({"version":1,"selected":path,"repositories":[path]}),
        )
        .unwrap();
        fs::write(&fixture.registry.path, &original).unwrap();
        let record = fixture.registry.record_for_path(&path).unwrap().unwrap();
        assert_eq!(record.id, legacy_id(&path));
        assert_eq!(
            fixture
                .registry
                .record_by_id(&record.id)
                .unwrap()
                .unwrap()
                .path,
            path
        );
        assert_eq!(fs::read(&fixture.registry.path).unwrap(), original);
        let updated = disable_account(&fixture.registry, &record);
        assert_eq!(updated.id, record.id);
        assert_eq!(fixture.registry.read().unwrap().records.len(), 1);
        let after = fs::read(&fixture.registry.path).unwrap();
        assert!(
            fixture
                .registry
                .compare_and_replace_account(
                    &record.id,
                    0,
                    StoredAccount {
                        revision: 1,
                        ..Default::default()
                    }
                )
                .is_err()
        );
        assert_eq!(fs::read(&fixture.registry.path).unwrap(), after);
    }

    #[test]
    fn worktree_switch_relocates_source_and_adopts_an_existing_target() {
        let fixture = RegistryFixture::new();
        let source = fixture.directory.join("source");
        let target = fixture.directory.join("target");
        let other = fixture.directory.join("other");
        fixture.registry.add(source.clone(), true).unwrap();
        let reviewed_source = fixture.registry.record_for_path(&source).unwrap().unwrap();
        let source_record = disable_account(&fixture.registry, &reviewed_source);
        let relocated = fixture
            .registry
            .switch_worktree_record(Some(&reviewed_source), target.clone())
            .unwrap();
        assert_eq!(relocated.id, source_record.id);
        assert_eq!(relocated.account.revision, 1);
        assert!(
            fixture
                .registry
                .switch_worktree_record(Some(&source_record), other.clone())
                .is_err()
        );
        assert!(matches!(
            relocated.account.assignment,
            crate::accounts::AccountAssignment::None {}
        ));
        assert!(fixture.registry.record_for_path(&source).unwrap().is_none());
        assert_eq!(
            fixture.registry.read().unwrap().repositories,
            vec![target.clone()]
        );
        assert!(
            fixture
                .registry
                .select_record(&source_record.id, &source)
                .is_err()
        );
        fixture.registry.add(other.clone(), false).unwrap();
        let other_record = fixture.registry.record_for_path(&other).unwrap().unwrap();
        let adopted = fixture
            .registry
            .switch_worktree(Some(&relocated.id), other.clone())
            .unwrap();
        assert_eq!(adopted.id, other_record.id);
        assert_eq!(adopted.account.revision, 0);
        assert_eq!(
            fixture
                .registry
                .record_by_id(&relocated.id)
                .unwrap()
                .unwrap()
                .path,
            target
        );
        assert_eq!(fixture.registry.read().unwrap().selected, Some(other));
    }

    #[test]
    fn forget_and_readd_never_revives_an_old_bound_record() {
        let fixture = RegistryFixture::new();
        let path = fixture.directory.join("repo");
        fixture.registry.add(path.clone(), true).unwrap();
        let old = fixture.registry.record_for_path(&path).unwrap().unwrap();
        fixture.registry.forget(&path).unwrap();
        assert!(fixture.registry.select_record(&old.id, &path).is_err());
        assert!(fixture.registry.record_by_id(&old.id).unwrap().is_none());
        fixture.registry.add(path.clone(), true).unwrap();
        let new = fixture.registry.record_for_path(&path).unwrap().unwrap();
        assert_ne!(new.id, old.id);
        assert!(
            fixture
                .registry
                .compare_and_replace_account(
                    &old.id,
                    0,
                    StoredAccount {
                        revision: 1,
                        ..Default::default()
                    }
                )
                .is_err()
        );
    }

    #[test]
    fn registry_rejects_duplicate_and_unrelated_records() {
        let fixture = RegistryFixture::new();
        let path = fixture.directory.join("repo");
        fixture.registry.add(path.clone(), true).unwrap();
        let mut data = fixture.registry.read().unwrap();
        data.records.push(data.records[0].clone());
        assert!(data.validate().is_err());
        data.records.pop();
        data.records[0].path = fixture.directory.join("unsaved");
        assert!(data.validate().is_err());
        data.records.clear();
        data.selected = Some(fixture.directory.join("unsaved"));
        assert!(data.validate().is_err());
    }

    #[test]
    fn malformed_present_account_records_never_default_to_effective() {
        let fixture = RegistryFixture::new();
        let path = fixture.directory.join("repo");
        fixture.registry.add(path, true).unwrap();
        let original = serde_json::to_value(fixture.registry.read().unwrap()).unwrap();
        let mut older = original.clone();
        older["records"][0]["account"]
            .as_object_mut()
            .unwrap()
            .remove("source_remote");
        let older_bytes = serde_json::to_vec(&older).unwrap();
        fs::write(&fixture.registry.path, &older_bytes).unwrap();
        let older = fixture.registry.read().unwrap();
        assert!(older.records[0].account.source_remote.is_none());
        assert_eq!(fs::read(&fixture.registry.path).unwrap(), older_bytes);
        for malformed in 0..5 {
            let mut data = original.clone();
            let record = data["records"][0].as_object_mut().unwrap();
            match malformed {
                0 => {
                    record.remove("account");
                }
                1 => {
                    record["account"]
                        .as_object_mut()
                        .unwrap()
                        .remove("assignment");
                }
                2 => {
                    record["account"]["assignment"] = serde_json::json!({"mode":"unknown"});
                }
                3 => {
                    record["account"]["assignment"] = serde_json::json!({"mode":"assigned","host":"github.com","login":"account"});
                }
                4 => {
                    record["account"]["assignment"] = serde_json::json!({"mode":"assigned","host":"github.com","login":"account","id":0});
                }
                _ => unreachable!(),
            }
            let bytes = serde_json::to_vec(&data).unwrap();
            fs::write(&fixture.registry.path, &bytes).unwrap();
            assert!(
                fixture.registry.read().is_err(),
                "malformed account case {malformed}"
            );
            assert_eq!(fs::read(&fixture.registry.path).unwrap(), bytes);
        }
    }

    #[cfg(unix)]
    #[test]
    fn record_paths_preserve_distinct_native_bytes() {
        use std::os::unix::ffi::OsStringExt;
        let fixture = RegistryFixture::new();
        let first = fixture
            .directory
            .join(std::ffi::OsString::from_vec(b"repo-\xff".to_vec()));
        let second = fixture
            .directory
            .join(std::ffi::OsString::from_vec(b"repo-\xfe".to_vec()));
        fixture.registry.add(first.clone(), true).unwrap();
        fixture.registry.add(second.clone(), false).unwrap();
        let left = fixture.registry.record_for_path(&first).unwrap().unwrap();
        let right = fixture.registry.record_for_path(&second).unwrap().unwrap();
        assert_ne!(left.id, right.id);
        assert_eq!(
            fixture
                .registry
                .record_by_id(&left.id)
                .unwrap()
                .unwrap()
                .path,
            first
        );
        assert_eq!(
            fixture
                .registry
                .record_by_id(&right.id)
                .unwrap()
                .unwrap()
                .path,
            second
        );
        assert_ne!(
            fixture.registry.namespace_id(),
            Registry::new(fixture.directory.join("other.json")).namespace_id()
        );
    }

    #[test]
    fn config_paths_honor_explicit_override_and_native_location() {
        let home = std::env::temp_dir().join("desktop-home");
        let local = std::env::temp_dir().join("desktop-local-app-data");
        let explicit = std::env::temp_dir().join("desktop-explicit");
        assert_eq!(
            config_directory(|key| match key {
                "XDG_CONFIG_HOME" => Some(explicit.clone().into_os_string()),
                _ => None,
            })
            .unwrap(),
            explicit
        );
        assert!(config_directory(|_| None).is_err());
        assert!(
            config_directory(|key| (key == "XDG_CONFIG_HOME").then(|| OsString::from("relative")))
                .is_err()
        );
        let native = config_directory(|key| match key {
            "XDG_CONFIG_HOME" => Some(OsString::new()),
            "HOME" => Some(home.clone().into_os_string()),
            "LOCALAPPDATA" => Some(local.clone().into_os_string()),
            _ => None,
        })
        .unwrap();
        #[cfg(target_os = "windows")]
        assert_eq!(native, local);
        #[cfg(target_os = "macos")]
        assert_eq!(native, home.join("Library/Application Support"));
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        assert_eq!(native, home.join(".config"));
    }
}
