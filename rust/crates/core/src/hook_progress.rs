//! Request-owned, bounded notifications about native Git hook execution.
use serde::Serialize;
use std::future::Future;
use tokio::sync::mpsc;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct HookProgress {
    pub checkpoint_id: String,
    pub hook_name: String,
    pub status: &'static str,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub enum CommitEvent {
    Hook(HookProgress),
    Output {
        checkpoint_id: String,
        stream: &'static str,
        data: String,
    },
}

tokio::task_local! { static SENDER: (mpsc::Sender<(u64, CommitEvent)>, u64); }

/// Attach progress to one authenticated request, never to a global subscriber.
pub async fn scope<F: Future>(
    sender: mpsc::Sender<(u64, CommitEvent)>,
    request_id: u64,
    work: F,
) -> F::Output {
    SENDER.scope((sender, request_id), work).await
}

pub(crate) fn sender() -> Option<(mpsc::Sender<(u64, CommitEvent)>, u64)> {
    SENDER.try_with(Clone::clone).ok()
}
