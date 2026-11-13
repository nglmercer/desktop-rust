//! Single-use capabilities for request-owned interactive hook decisions.
//!
//! Capabilities are sent only to the transport that owns a running operation.
//! Missing, expired, duplicated, or malformed answers never authorize a hook.
use anyhow::{Result, ensure};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};
use tokio::sync::oneshot;
use uuid::Uuid;

const MAX_PENDING: usize = 128;
type Decisions = HashMap<Uuid, oneshot::Sender<bool>>;
static PENDING: OnceLock<Mutex<Decisions>> = OnceLock::new();
fn table() -> &'static Mutex<Decisions> {
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) struct PendingDecision {
    id: Uuid,
    receiver: Option<oneshot::Receiver<bool>>,
}
impl PendingDecision {
    pub(crate) fn new() -> Result<Self> {
        let mut pending = table().lock().unwrap_or_else(|error| error.into_inner());
        ensure!(
            pending.len() < MAX_PENDING,
            "Too many pending hook decisions"
        );
        let id = Uuid::new_v4();
        let (sender, receiver) = oneshot::channel();
        ensure!(
            !pending.contains_key(&id),
            "Hook decision identifier collision"
        );
        pending.insert(id, sender);
        Ok(Self {
            id,
            receiver: Some(receiver),
        })
    }
    pub(crate) fn id(&self) -> String {
        self.id.to_string()
    }
    pub(crate) async fn wait(&mut self) -> bool {
        let Some(receiver) = self.receiver.take() else {
            return false;
        };
        let cancellation = crate::cancellation::Cancellation::current();
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => false,
            // Git's overall 120-second deadline starts before the hook runs,
            // so the decision may have less time than this local upper bound.
            _ = tokio::time::sleep(std::time::Duration::from_secs(120)) => false,
            value = receiver => value.unwrap_or(false),
        }
    }
}
impl Drop for PendingDecision {
    fn drop(&mut self) {
        table()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.id);
    }
}

/// Resolve a single capability. Invalid or already used capabilities fail closed.
pub fn resolve(id: &str, ignore_failure: bool) -> bool {
    let Ok(id) = Uuid::parse_str(id) else {
        return false;
    };
    let sender = table()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&id);
    sender.is_some_and(|sender| sender.send(ignore_failure).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn decisions_are_one_shot_and_removed_on_drop() {
        let mut decision = PendingDecision::new().unwrap();
        let id = decision.id();
        assert!(!resolve("malformed", true));
        assert!(resolve(&id, true));
        assert!(!resolve(&id, true));
        assert!(decision.wait().await);
        let decision = PendingDecision::new().unwrap();
        let id = decision.id();
        drop(decision);
        assert!(!resolve(&id, true));
    }
    #[tokio::test]
    async fn cancellation_rejects_a_pending_decision() {
        let cancel = crate::cancellation::Cancellation::default();
        let mut decision = PendingDecision::new().unwrap();
        let id = decision.id();
        cancel.cancel();
        assert!(!cancel.scope(decision.wait()).await);
        drop(decision);
        assert!(!resolve(&id, true));
    }
}
