//! Request-session-owned, bounded immutable comparison output.
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    future::Future,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

/// Mirrors the renderer's MaxDiffBufferSize (70 MB decimal).
pub(crate) const MAX_BYTES: usize = 70_000_000;
// Two concurrent comparisons, each with at most MAX_BYTES + 1 retained
// output bytes. Rejection avoids deadlocking a serialized transport whose
// queued chunk requests would otherwise wait behind a snapshot reservation.
const MAX_SNAPSHOTS: usize = 2;
const MAX_CHUNK: usize = 512 * 1024;
const TTL: Duration = Duration::from_secs(300);
tokio::task_local! { static OWNER: Arc<Owner>; }

/// Transport-owned identity, never supplied in an RPC payload.
pub struct Owner {
    id: Uuid,
    closed: AtomicBool,
    closed_signal: tokio::sync::watch::Sender<bool>,
    reserved: AtomicBool,
}
impl Owner {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            id: Uuid::new_v4(),
            closed: AtomicBool::new(false),
            closed_signal: tokio::sync::watch::channel(false).0,
            reserved: AtomicBool::new(false),
        })
    }
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.closed_signal.send_replace(true);
        if let Ok(mut map) = entries().lock() {
            map.retain(|_, entry| entry.owner != self.id);
        }
    }
}

struct Entry {
    owner: Uuid,
    repository: String,
    bytes: Vec<u8>,
    expires: Instant,
    idle_ttl: Duration,
    expiry_task: Option<tokio::task::AbortHandle>,
    permit: Option<Reservation>,
    upload: bool,
}
impl Drop for Entry {
    fn drop(&mut self) {
        if let Some(task) = &self.expiry_task {
            task.abort();
        }
    }
}

pub(crate) struct Reservation {
    owner: Arc<Owner>,
    _permit: Option<OwnedSemaphorePermit>,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.owner.reserved.store(false, Ordering::Release);
    }
}
static ENTRIES: OnceLock<Mutex<HashMap<Uuid, Entry>>> = OnceLock::new();
static CAPACITY: OnceLock<Arc<Semaphore>> = OnceLock::new();
fn entries() -> &'static Mutex<HashMap<Uuid, Entry>> {
    ENTRIES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The transport chooses the owner; no request field can set it.
pub async fn scope<F: Future>(owner: Arc<Owner>, work: F) -> F::Output {
    OWNER.scope(owner, work).await
}

pub(crate) async fn reserve() -> Result<Reservation> {
    let owner = OWNER
        .try_with(Arc::clone)
        .context("Comparison snapshots require an authenticated session")?;
    ensure!(
        !owner.closed.load(Ordering::SeqCst),
        "Comparison session expired"
    );
    ensure!(
        owner
            .reserved
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok(),
        "A comparison is already being read by this session"
    );
    let mut reservation = Reservation {
        owner: owner.clone(),
        _permit: None,
    };
    let semaphore = CAPACITY
        .get_or_init(|| Arc::new(Semaphore::new(MAX_SNAPSHOTS)))
        .clone();
    let mut closed = owner.closed_signal.subscribe();
    let cancellation = crate::cancellation::Cancellation::current();
    reservation._permit = Some(tokio::select! {
        biased;
        _ = cancellation.cancelled() => return Err(anyhow::anyhow!(cancellation.message())),
        _ = closed.wait_for(|closed| *closed) => return Err(anyhow::anyhow!("Comparison session expired")),
        permit = semaphore.acquire_owned() => permit?,
    });
    ensure!(
        !owner.closed.load(Ordering::SeqCst),
        "Comparison session expired"
    );
    Ok(reservation)
}

pub(crate) fn insert(repository: &str, bytes: Vec<u8>, permit: Reservation) -> Result<Value> {
    insert_with_ttl(repository, bytes, permit, TTL, false)
}

pub(crate) fn insert_upload(repository: &str, permit: Reservation) -> Result<Value> {
    insert_with_ttl(repository, Vec::new(), permit, TTL, true)
}

fn insert_with_ttl(
    repository: &str,
    bytes: Vec<u8>,
    permit: Reservation,
    ttl: Duration,
    upload: bool,
) -> Result<Value> {
    ensure!(
        bytes.len() <= MAX_BYTES,
        "Comparison exceeds the renderable limit"
    );
    let owner = OWNER
        .try_with(Arc::clone)
        .context("Comparison session expired")?;
    let id = Uuid::new_v4();
    let size = bytes.len();
    let mut map = entries()
        .lock()
        .map_err(|_| anyhow::anyhow!("Comparison snapshot unavailable"))?;
    ensure!(
        !owner.closed.load(Ordering::SeqCst),
        "Comparison session expired"
    );
    map.insert(
        id,
        Entry {
            owner: owner.id,
            repository: repository.to_owned(),
            bytes,
            expires: Instant::now() + ttl,
            idle_ttl: ttl,
            expiry_task: None,
            permit: Some(permit),
            upload,
        },
    );
    // Actively reclaim even if the client disappears without another request.
    let task = tokio::spawn(async move {
        tokio::time::sleep(ttl).await;
        loop {
            let delay = match entries().lock() {
                Ok(mut map) => match map.get(&id) {
                    Some(entry) if entry.expires > Instant::now() => {
                        entry.expires.saturating_duration_since(Instant::now())
                    }
                    Some(_) => {
                        map.remove(&id);
                        return;
                    }
                    None => return,
                },
                Err(_) => return,
            };
            tokio::time::sleep(delay).await;
        }
    });
    map.get_mut(&id)
        .expect("snapshot was just inserted")
        .expiry_task = Some(task.abort_handle());
    drop(map);
    Ok(json!({"kind":"snapshot", "id":id.to_string(), "size":size}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn uploads_are_ordered_private_and_released_on_close() {
        let owner = Owner::new();
        let other = Owner::new();
        let id = scope(owner.clone(), async {
            let permit = reserve().await.unwrap();
            let upload = insert_upload("repo", permit).unwrap();
            let id = Uuid::parse_str(upload["id"].as_str().unwrap()).unwrap();
            assert!(append_upload("repo", id, 1, "YQ==").is_err());
            assert!(chunk("repo", id, 0, 1).is_err());
            assert_eq!(append_upload("repo", id, 0, "YQ==").unwrap()["size"], 1);
            assert!(take_upload("repo", id, 2).is_err());
            id
        })
        .await;
        scope(other.clone(), async {
            assert!(append_upload("repo", id, 1, "Yg==").is_err());
            assert!(take_upload("repo", id, 1).is_err());
            assert!(release_owned(&id.to_string()).is_err());
        })
        .await;
        owner.close();
        scope(owner.clone(), async {
            assert!(take_upload("repo", id, 1).is_err());
        })
        .await;
        other.close();
    }

    #[tokio::test]
    async fn snapshots_are_session_owned_and_actively_reclaimed() {
        let owner = Owner::new();
        let other = Owner::new();
        let third = Owner::new();
        let id = scope(owner.clone(), async {
            let permit = reserve().await.unwrap();
            let result = insert_with_ttl(
                "repo",
                b"abc\0def".to_vec(),
                permit,
                Duration::from_millis(400),
                false,
            )
            .unwrap();
            let id = Uuid::parse_str(result["id"].as_str().unwrap()).unwrap();
            assert_eq!(
                chunk("repo", id, 1, 3).unwrap()["data"],
                STANDARD.encode(b"bc\0")
            );
            assert!(chunk("other", id, 0, 3).is_err());
            id
        })
        .await;
        scope(other.clone(), async {
            assert!(chunk("repo", id, 0, 3).is_err());
            assert!(release("repo", id).is_err());
            assert!(release_owned(&id.to_string()).is_err());
            let second = reserve().await.unwrap();
            let second = insert("repo", vec![2], second).unwrap();
            let second = Uuid::parse_str(second["id"].as_str().unwrap()).unwrap();
            assert!(reserve().await.is_err());
            assert!(
                scope(
                    third.clone(),
                    tokio::time::timeout(Duration::from_millis(10), reserve())
                )
                .await
                .is_err()
            );
            let cancellation = crate::cancellation::Cancellation::default();
            cancellation.cancel();
            assert!(
                scope(third.clone(), cancellation.clone().scope(reserve()))
                    .await
                    .is_err()
            );
            let waiting_owner = Owner::new();
            let waiter = tokio::spawn(scope(waiting_owner.clone(), async {
                reserve().await.is_err()
            }));
            tokio::task::yield_now().await;
            waiting_owner.close();
            assert!(
                tokio::time::timeout(Duration::from_millis(100), waiter)
                    .await
                    .unwrap()
                    .unwrap()
            );
            release("repo", second).unwrap();
        })
        .await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        scope(owner.clone(), async {
            assert!(chunk("repo", id, 0, 1).is_ok());
        })
        .await;
        tokio::time::sleep(Duration::from_millis(250)).await;
        scope(owner.clone(), async {
            assert!(chunk("repo", id, 0, 1).is_ok());
        })
        .await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        scope(other.clone(), async {
            assert!(chunk("repo", id, 0, 3).is_err());
            drop(reserve().await.unwrap());
            let permit = reserve().await.unwrap();
            let result = insert("repo", vec![1], permit).unwrap();
            let id = Uuid::parse_str(result["id"].as_str().unwrap()).unwrap();
            let task = entries()
                .lock()
                .unwrap()
                .get(&id)
                .unwrap()
                .expiry_task
                .clone()
                .unwrap();
            release_owned(&id.to_string()).unwrap();
            tokio::task::yield_now().await;
            assert!(task.is_finished());
        })
        .await;
        scope(owner.clone(), async {
            let permit = reserve().await.unwrap();
            owner.close();
            assert!(insert("repo", vec![1], permit).is_err());
        })
        .await;
        scope(other.clone(), async {
            drop(reserve().await.unwrap());
        })
        .await;
        other.close();
        third.close();
    }
}

pub(crate) fn chunk(repository: &str, id: Uuid, offset: u64, length: u32) -> Result<Value> {
    let owner = OWNER
        .try_with(|value| value.id)
        .context("Comparison session expired")?;
    ensure!(
        length > 0 && length as usize <= MAX_CHUNK,
        "Invalid comparison chunk size"
    );
    let mut map = entries()
        .lock()
        .map_err(|_| anyhow::anyhow!("Comparison snapshot unavailable"))?;
    if map
        .get(&id)
        .is_some_and(|entry| entry.expires <= Instant::now())
    {
        map.remove(&id);
    }
    let entry = map.get_mut(&id).context("Comparison snapshot expired")?;
    ensure!(
        entry.owner == owner && entry.repository == repository,
        "Comparison snapshot does not belong to this session"
    );
    ensure!(!entry.upload, "Comparison upload is not readable");
    entry.expires = Instant::now() + entry.idle_ttl;
    let start = usize::try_from(offset).context("Invalid comparison offset")?;
    ensure!(start <= entry.bytes.len(), "Invalid comparison offset");
    let end = start.saturating_add(length as usize).min(entry.bytes.len());
    Ok(
        json!({"offset":start,"size":entry.bytes.len(),"data":STANDARD.encode(&entry.bytes[start..end])}),
    )
}

/// Append one bounded, ordered input chunk to this session's upload.
pub(crate) fn append_upload(repository: &str, id: Uuid, offset: u64, data: &str) -> Result<Value> {
    ensure!(
        data.len() <= 64 * 1024,
        "Comparison upload chunk exceeds its limit"
    );
    let bytes = STANDARD
        .decode(data)
        .context("Invalid comparison upload encoding")?;
    ensure!(
        bytes.len() <= 48 * 1024,
        "Comparison upload chunk exceeds its limit"
    );
    let owner = OWNER
        .try_with(|value| value.id)
        .context("Comparison session expired")?;
    let mut map = entries()
        .lock()
        .map_err(|_| anyhow::anyhow!("Comparison snapshot unavailable"))?;
    if map
        .get(&id)
        .is_some_and(|entry| entry.expires <= Instant::now())
    {
        map.remove(&id);
    }
    let entry = map.get_mut(&id).context("Comparison upload expired")?;
    ensure!(
        entry.owner == owner && entry.repository == repository && entry.upload,
        "Comparison upload does not belong to this session"
    );
    ensure!(
        offset == entry.bytes.len() as u64,
        "Comparison upload offset changed"
    );
    ensure!(
        entry.bytes.len().saturating_add(bytes.len()) <= MAX_BYTES,
        "Comparison upload exceeds its limit"
    );
    entry.bytes.extend_from_slice(&bytes);
    entry.expires = Instant::now() + entry.idle_ttl;
    Ok(json!({"size":entry.bytes.len()}))
}

/// Consume an upload without releasing its capacity reservation.
pub(crate) fn take_upload(repository: &str, id: Uuid, size: u64) -> Result<(Vec<u8>, Reservation)> {
    let owner = OWNER
        .try_with(|value| value.id)
        .context("Comparison session expired")?;
    let mut map = entries()
        .lock()
        .map_err(|_| anyhow::anyhow!("Comparison snapshot unavailable"))?;
    if map
        .get(&id)
        .is_some_and(|entry| entry.expires <= Instant::now())
    {
        map.remove(&id);
    }
    let entry = map.get(&id).context("Comparison upload expired")?;
    ensure!(
        entry.owner == owner && entry.repository == repository && entry.upload,
        "Comparison upload does not belong to this session"
    );
    ensure!(
        entry.bytes.len() as u64 == size,
        "Comparison upload is incomplete"
    );
    let mut entry = map.remove(&id).expect("entry was just checked");
    let bytes = std::mem::take(&mut entry.bytes);
    let permit = entry
        .permit
        .take()
        .context("Comparison upload lost its reservation")?;
    Ok((bytes, permit))
}

pub(crate) fn release(repository: &str, id: Uuid) -> Result<Value> {
    let owner = OWNER
        .try_with(|value| value.id)
        .context("Comparison session expired")?;
    let mut map = entries()
        .lock()
        .map_err(|_| anyhow::anyhow!("Comparison snapshot unavailable"))?;
    if let Some(entry) = map.get(&id) {
        ensure!(
            entry.owner == owner && entry.repository == repository,
            "Comparison snapshot does not belong to this session"
        );
    }
    map.remove(&id);
    Ok(json!({"released":true}))
}

/// Release a session's snapshot after the selected repository changes.
pub fn release_owned(id: &str) -> Result<Value> {
    let id = Uuid::parse_str(id).context("Invalid comparison snapshot id")?;
    let owner = OWNER
        .try_with(|value| value.id)
        .context("Comparison session expired")?;
    let mut map = entries()
        .lock()
        .map_err(|_| anyhow::anyhow!("Comparison snapshot unavailable"))?;
    if let Some(entry) = map.get(&id) {
        ensure!(
            entry.owner == owner,
            "Comparison snapshot does not belong to this session"
        );
    }
    map.remove(&id);
    Ok(json!({"released":true}))
}
