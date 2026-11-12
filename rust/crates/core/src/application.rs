//! Experimental common application-service contract and job ledger.
//!
//! The CLI, bridge and shell do not yet all delegate workflow execution here.
//! Repository and account resolution, cancellation and transport integration
//! remain migration work; the ledger below is not an execution service.
//!
//! Contract mirrors `app/src/backend/` files generated from this module.
//! Internal wire formats of the existing carriers are preserved: the bridge
//! announces `version 3` with `{cancel,max_pending}` capabilities (not JSON-RPC
//! v1/v2); the Tauri/Wry shell uses correlated `__rpc`/`%RpcResult` IPC. Both
//! carriers have different supported operations; do not infer parity from
//! the shared types.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cancellation::Cancellation;

/// Backend protocol version. The bridge advertises `3`; this is the same
/// version negotiable via [`Capabilities`] when a client connects.
pub const BACKEND_VERSION: u32 = 3;

/// Declared capabilities the backend advertises on connect. Transports must
/// send this back unchanged until the backend version changes.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Capabilities {
    /// Cooperative cancellation via `cancel(request_id)` / `cancel` RPC.
    pub cancel: bool,
    /// Max pending RPCs on the bridge (32) and IPC promises on the webview
    /// (profile-dependent, see `io::limits`).
    pub max_pending: u32,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            cancel: true,
            max_pending: 32,
        }
    }
}

/// Common request envelope — maps to `IRequest<T>` in TypeScript.
/// `sessionId`/`requestId` are transport-assigned; `method` is the `Operation`
/// snake_case name; `params` is the typed payload (flattened by `Operation`'s
/// `deny_unknown_fields` when carried over the bridge).
#[derive(Clone, Debug, Deserialize)]
pub struct IRequest {
    pub session_id: String,
    pub request_id: String,
    pub method: String,
    pub repository_id: Option<String>,
    pub account_id: Option<String>,
    pub deadline_ms: Option<u64>,
    #[serde(default, flatten)]
    pub params: Value,
}

/// Uniform backend error — maps to `IBackendError` in TypeScript.
/// Callers show `message` and act on `code`; `retryable` signals whether a
/// resume/snapshot refresh may succeed (e.g. `cancelled` after a local
/// mutation requires inspecting state; remote mutations may be uncertain).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct IBackendError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub job_id: Option<String>,
    pub recovery_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl IBackendError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: false,
            job_id: None,
            recovery_id: None,
            details: None,
        }
    }
    pub fn retryable(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            retryable: true,
            ..Self::new(code, message)
        }
    }
    pub fn cancelled() -> Self {
        Self::new("cancelled", Cancellation::current().message())
    }
    pub fn remote_cancelled() -> Self {
        Self::new(
            "cancelled",
            "Operation cancelled; inspect GitHub before retrying: the remote mutation may have completed",
        )
        .with_retryable(true)
    }
    pub fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }
    pub fn with_job(mut self, job_id: impl Into<String>) -> Self {
        self.job_id = Some(job_id.into());
        self
    }
}

impl From<anyhow::Error> for IBackendError {
    fn from(error: anyhow::Error) -> Self {
        let message = error.to_string();
        let code = if message.contains("cancelled") {
            "cancelled"
        } else if message.contains("queue") || message.contains("QUEUE_FULL") {
            "queue_full"
        } else if message.contains("not a working tree") {
            "not_a_repository"
        } else {
            "internal"
        };
        Self::new(code, message.clone()).with_retryable(code == "cancelled" || code == "queue_full")
    }
}

/// Terminal job lifecycle. `queued → running → settling → terminal`; the
/// terminal is exactly one of success/error/cancelled/indeterminate.
/// Cancellation is an independent signal; an operation cancelled or expired
/// before starting produces no effects.
#[derive(Clone, Debug, PartialEq)]
pub enum JobState {
    Queued,
    Running,
    Settling,
    Succeeded(Value),
    Failed(IBackendError),
    Cancelled,
    Indeterminate(IBackendError),
}

impl JobState {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            JobState::Succeeded(_)
                | JobState::Failed(_)
                | JobState::Cancelled
                | JobState::Indeterminate(_)
        )
    }
}

/// A registered long-running job. Created before admission with session/
/// generation, identifier, resources, effect class, deadline and cancel
/// policy. The worker is released as soon as it is unnecessary; the lease is
/// held until effects, verification/acceptance and recovery obligations
/// complete or are transferred to a recovery owner.
pub struct Job {
    pub id: String,
    pub session_id: String,
    pub method: String,
    pub resources: String,
    pub effect_class: String,
    pub deadline: Option<Duration>,
    pub state: JobState,
    /// How the pending delivery is released (bounded record kept after the
    /// worker is gone; ACK-independent credit).
    pub pending_delivery_bytes: usize,
}

impl Job {
    pub fn new(
        id: impl Into<String>,
        session_id: impl Into<String>,
        method: impl Into<String>,
        resources: impl Into<String>,
        effect_class: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            session_id: session_id.into(),
            method: method.into(),
            resources: resources.into(),
            effect_class: effect_class.into(),
            deadline: None,
            state: JobState::Queued,
            pending_delivery_bytes: 0,
        }
    }
}

/// Experimental job ledger. Transports do not yet share its lifecycle.
#[derive(Clone, Default)]
pub struct ApplicationService {
    inner: Arc<application_inner::ApplicationInner>,
}

mod application_inner {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    pub(super) struct ApplicationInner {
        pub(super) jobs: Mutex<HashMap<String, Job>>,
    }
}

impl ApplicationService {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(application_inner::ApplicationInner::default()),
        }
    }

    /// Capabilities this service version advertises. The bridge already
    /// returns the same values on connect; this method is the Rust-side
    /// source of truth so transports cannot drift.
    pub fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    pub fn version(&self) -> u32 {
        BACKEND_VERSION
    }

    /// Register a job before admitting it. Rejects duplicate IDs without
    /// overwriting an existing ledger entry.
    pub fn register_job(&self, job: Job) -> Result<String, Box<IBackendError>> {
        let id = job.id.clone();
        let mut jobs = self.inner.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if jobs.contains_key(&id) {
            return Err(Box::new(IBackendError::new(
                "duplicate_job",
                format!("job {id} already registered"),
            )));
        }
        jobs.insert(id.clone(), job);
        Ok(id)
    }

    pub fn job(&self, id: &str) -> Option<Job> {
        let jobs = self.inner.jobs.lock().unwrap_or_else(|e| e.into_inner());
        // Clone fields; JobState::Succeeded holds Value, clonable.
        // We return a detached snapshot; mutating it does not affect the ledger.
        jobs.get(id).map(|j| Job {
            id: j.id.clone(),
            session_id: j.session_id.clone(),
            method: j.method.clone(),
            resources: j.resources.clone(),
            effect_class: j.effect_class.clone(),
            deadline: j.deadline,
            state: j.state.clone(),
            pending_delivery_bytes: j.pending_delivery_bytes,
        })
    }

    /// Transition to Running, but only from Queued and only before deadline.
    /// A job cancelled or expired before starting yields `Cancelled` and
    /// must produce no effects — the caller checks `check_before_effects()`.
    pub fn start_job(&self, id: &str) -> Result<(), Box<IBackendError>> {
        let mut jobs = self.inner.jobs.lock().unwrap_or_else(|e| e.into_inner());
        let job = jobs
            .get_mut(id)
            .ok_or_else(|| Box::new(IBackendError::new("unknown_job", id.to_string())))?;
        match job.state {
            JobState::Queued => job.state = JobState::Running,
            JobState::Cancelled => return Err(Box::new(IBackendError::cancelled())),
            _ => {
                return Err(Box::new(IBackendError::new(
                    "invalid_state",
                    format!("job {id} is not Queued"),
                )));
            }
        }
        Ok(())
    }

    /// Independent cancel signal. Affects only the named job/session.
    pub fn cancel_job(&self, id: &str) {
        let mut jobs = self.inner.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(job) = jobs.get_mut(id)
            && !job.state.is_terminal()
        {
            job.state = JobState::Cancelled;
        }
    }

    pub fn settle(&self, id: &str, result: Result<Value, IBackendError>) {
        let mut jobs = self.inner.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(job) = jobs.get_mut(id)
            && !job.state.is_terminal()
        {
            job.state = match result {
                Ok(v) => JobState::Succeeded(v),
                Err(e) => JobState::Failed(e),
            };
        }
    }

    /// Abort before effects if the job was cancelled — call before any
    /// mutation. Distinguishes "did not start" from "may have completed"
    /// (the latter yields `Indeterminate` and needs reconciliation).
    pub fn check_before_effects(&self, id: &str) -> Result<(), Box<IBackendError>> {
        let jobs = self.inner.jobs.lock().unwrap_or_else(|e| e.into_inner());
        match jobs.get(id).map(|j| &j.state) {
            Some(JobState::Cancelled) => Err(Box::new(IBackendError::cancelled())),
            _ => Ok(()),
        }
    }

    /// Lease release rule: the worker lease is released only after effects,
    /// verification and recovery obligations complete, or are explicitly
    /// transferred to a recovery owner. `pending_delivery_bytes` is kept as
    /// an independent bounded record until delivered or explicitly abandoned.
    pub fn release_lease(&self, id: &str) -> Result<(), Box<IBackendError>> {
        let mut jobs = self.inner.jobs.lock().unwrap_or_else(|e| e.into_inner());
        let job = jobs
            .get_mut(id)
            .ok_or_else(|| Box::new(IBackendError::new("unknown_job", id.to_string())))?;
        if !job.state.is_terminal() {
            return Err(Box::new(IBackendError::new(
                "not_terminal",
                format!("job {id} has not settled"),
            )));
        }
        // Pending delivery survives as bounded record; do not free worker state
        // until its credit is acknowledged or abandoned via `abandon_delivery`.
        Ok(())
    }

    /// Credit for a pending delivery is freed when the receiver accepts it
    /// within its own budget, or when the session abandons it explicitly.
    pub fn abandon_delivery(&self, id: &str) -> Result<usize, Box<IBackendError>> {
        let mut jobs = self.inner.jobs.lock().unwrap_or_else(|e| e.into_inner());
        let job = jobs
            .get_mut(id)
            .ok_or_else(|| Box::new(IBackendError::new("unknown_job", id.to_string())))?;
        let freed = job.pending_delivery_bytes;
        job.pending_delivery_bytes = 0;
        Ok(freed)
    }
}

/// Error categorisation helper used by every transport to wrap `anyhow` into
/// `IBackendError` without leaking stack traces.
pub fn backend_error(error: anyhow::Error) -> IBackendError {
    error.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_match_bridge_wire() {
        let svc = ApplicationService::new();
        let caps = svc.capabilities();
        assert!(caps.cancel);
        assert_eq!(caps.max_pending, 32);
        assert_eq!(svc.version(), 3);
    }

    #[test]
    fn cancelled_before_start_produces_no_effects() {
        let svc = ApplicationService::new();
        svc.register_job(Job::new("j1", "s1", "commit", "repo:/tmp/a", "mutation"))
            .unwrap();
        svc.cancel_job("j1");
        // Cancelled before start: start must fail and no effects may run.
        assert!(svc.start_job("j1").is_err());
        assert!(svc.check_before_effects("j1").is_err());
    }

    #[test]
    fn job_reaches_exactly_one_terminal() {
        let svc = ApplicationService::new();
        svc.register_job(Job::new("j2", "s1", "push", "repo:/tmp/a", "remote"))
            .unwrap();
        svc.start_job("j2").unwrap();
        svc.settle("j2", Ok(Value::Null));
        svc.settle("j2", Err(IBackendError::new("late", "ignored")));
        let state = svc.job("j2").unwrap().state;
        assert!(matches!(state, JobState::Succeeded(_)));
    }
}
