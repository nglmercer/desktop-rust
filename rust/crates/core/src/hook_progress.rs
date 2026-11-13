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

tokio::task_local! { static SENDER: (mpsc::Sender<(u64, HookProgress)>, u64); }

/// Attach progress to one authenticated request, never to a global subscriber.
pub async fn scope<F: Future>(
    sender: mpsc::Sender<(u64, HookProgress)>,
    request_id: u64,
    work: F,
) -> F::Output {
    SENDER.scope((sender, request_id), work).await
}

#[cfg(unix)]
pub(crate) fn sender() -> Option<(mpsc::Sender<(u64, HookProgress)>, u64)> {
    SENDER.try_with(Clone::clone).ok()
}
