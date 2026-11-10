//! State ownership, IPC dispatch and domain commands. Replaces `AppStore`,
//! `Dispatcher` and `repository-state-cache.ts`.

pub mod commands;
pub mod dispatcher;

pub use dispatcher::{Outcome, invoke, send};
