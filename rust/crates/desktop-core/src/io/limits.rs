//! Finite resource budgets for the host services.
//!
//! Every limit lives in [`Limits`] and is selected by a profile. The profile
//! comes from the `DESKTOP_RESOURCE_PROFILE` environment variable (`default`
//! or `low-memory`); the UI has no control for it.
//!
//! Memory that is retained on behalf of a request or a stream is charged
//! against a [`ByteBudget`]. A [`Permit`] gives its bytes back exactly once,
//! when it is dropped, so success, error, cancellation and shutdown all release
//! the same amount.

use std::sync::{Arc, Condvar, Mutex};

/// The two budgets documented for this phase. Starting points, not benchmarks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Default,
    LowMemory,
}

impl Profile {
    /// Reads `DESKTOP_RESOURCE_PROFILE`; anything but `low-memory` is `default`.
    pub fn from_env() -> Self {
        match std::env::var("DESKTOP_RESOURCE_PROFILE").as_deref() {
            Ok("low-memory") => Self::LowMemory,
            _ => Self::Default,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::LowMemory => "low-memory",
        }
    }
}

pub const KIB: usize = 1024;
pub const MIB: usize = 1024 * KIB;

/// Upper bound for one binary block on a stream, in raw bytes.
pub const STREAM_BLOCK: usize = 64 * KIB;

/// Limits resolved for one profile. Field names follow the documented table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub profile: Profile,
    /// Workers for short IO (metadata, ranges, small file operations).
    pub short_workers: usize,
    /// Workers for long operations (git, HTTP, hashing, recursive work).
    pub long_workers: usize,
    /// Persistent interactive processes alive at the same time.
    pub process_slots: usize,
    /// Normal requests waiting in one executor queue.
    pub queued_requests: usize,
    /// Payload bytes retained by one executor queue.
    pub queued_bytes: usize,
    /// Input bytes pending per process, counting JavaScript and Rust together.
    pub process_input: usize,
    /// Output bytes in flight per process, split evenly between stdout and stderr.
    pub process_output: usize,
    /// Global output bytes in flight across all processes.
    pub stream_output: usize,
    /// Global input bytes pending across all processes.
    pub stream_input: usize,
    /// Normal IPC promises that JavaScript may keep pending.
    pub pending_promises: usize,
    /// Retained stdout/stderr of one git command, beyond which it is cut.
    pub git_output: usize,
    /// Response body retained for one HTTP request.
    pub http_response: usize,
}

impl Limits {
    pub fn for_profile(profile: Profile) -> Self {
        let cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        match profile {
            Profile::Default => Self {
                profile,
                short_workers: cpus.clamp(1, 4),
                long_workers: 2,
                process_slots: 2,
                queued_requests: 64,
                queued_bytes: 8 * MIB,
                process_input: MIB,
                process_output: MIB,
                stream_output: 4 * MIB,
                stream_input: 4 * MIB,
                pending_promises: 128,
                git_output: 64 * MIB,
                http_response: 64 * MIB,
            },
            Profile::LowMemory => Self {
                profile,
                short_workers: cpus.clamp(1, 2),
                long_workers: 1,
                process_slots: 1,
                queued_requests: 32,
                queued_bytes: 4 * MIB,
                process_input: 512 * KIB,
                process_output: 512 * KIB,
                stream_output: 2 * MIB,
                stream_input: 2 * MIB,
                pending_promises: 64,
                git_output: 16 * MIB,
                http_response: 16 * MIB,
            },
        }
    }

    /// Bytes of output that one stream (stdout or stderr) may have in flight.
    pub fn stream_window(&self) -> usize {
        self.process_output / 2
    }

    /// Input bytes JavaScript may hold before Rust has taken a block.
    pub fn js_input_budget(&self) -> usize {
        self.process_input.saturating_sub(STREAM_BLOCK)
    }
}

/// Byte accounting with blocking and non-blocking acquisition.
pub struct ByteBudget {
    capacity: usize,
    state: Mutex<BudgetState>,
    released: Condvar,
}

struct BudgetState {
    used: usize,
    closed: bool,
}

impl ByteBudget {
    pub fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            capacity,
            state: Mutex::new(BudgetState {
                used: 0,
                closed: false,
            }),
            released: Condvar::new(),
        })
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn used(&self) -> usize {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).used
    }

    /// Reserve `bytes` without waiting. `None` when the budget is full or closed.
    pub fn try_acquire(self: &Arc<Self>, bytes: usize) -> Option<Permit> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed || bytes > self.capacity || state.used + bytes > self.capacity {
            return None;
        }
        state.used += bytes;
        Some(Permit {
            budget: self.clone(),
            bytes,
        })
    }

    /// Reserve `bytes`, waiting for other permits to be released. `None` once
    /// the budget is closed or the request can never fit.
    pub fn acquire(self: &Arc<Self>, bytes: usize) -> Option<Permit> {
        if bytes > self.capacity {
            return None;
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if state.closed {
                return None;
            }
            if state.used + bytes <= self.capacity {
                state.used += bytes;
                return Some(Permit {
                    budget: self.clone(),
                    bytes,
                });
            }
            state = self.released.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Wake every waiter and refuse further acquisitions. Used on shutdown.
    pub fn close(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
        self.released.notify_all();
    }

    fn release(&self, bytes: usize) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.used = state.used.saturating_sub(bytes);
        drop(state);
        self.released.notify_all();
    }
}

/// A reservation against a [`ByteBudget`]. Dropping it releases the bytes.
pub struct Permit {
    budget: Arc<ByteBudget>,
    bytes: usize,
}

impl Permit {
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Give back everything above `bytes`, for a reservation that turned out
    /// larger than what was actually used.
    pub fn shrink_to(&mut self, bytes: usize) {
        if bytes < self.bytes {
            self.budget.release(self.bytes - bytes);
            self.bytes = bytes;
        }
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        if self.bytes > 0 {
            self.budget.release(self.bytes);
        }
    }
}

/// Shared budgets owned by one application instance.
pub struct Resources {
    pub limits: Limits,
    /// One unit per live persistent process.
    pub process_slots: Arc<ByteBudget>,
    /// Global input bytes pending across processes.
    pub input: Arc<ByteBudget>,
    /// Global output bytes in flight across processes.
    pub output: Arc<ByteBudget>,
    /// Bytes retained for whole results (git output, HTTP bodies) at once.
    pub results: Arc<ByteBudget>,
}

impl Resources {
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            process_slots: ByteBudget::new(limits.process_slots),
            input: ByteBudget::new(limits.stream_input),
            output: ByteBudget::new(limits.stream_output),
            results: ByteBudget::new(limits.git_output),
        }
    }

    /// Profile from the environment, with the default machine limits.
    pub fn from_env() -> Self {
        Self::new(Limits::for_profile(Profile::from_env()))
    }

    /// Stop admitting stream data and wake any blocked reader or writer.
    pub fn close(&self) {
        self.process_slots.close();
        self.input.close();
        self.output.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permits_release_once_when_dropped() {
        let budget = ByteBudget::new(10);
        let first = budget.try_acquire(6).expect("fits");
        assert!(budget.try_acquire(5).is_none(), "6 + 5 exceeds 10");
        assert_eq!(budget.used(), 6);
        drop(first);
        assert_eq!(budget.used(), 0);
        assert!(budget.try_acquire(10).is_some());
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn shrinking_returns_only_the_unused_part() {
        let budget = ByteBudget::new(10);
        let mut permit = budget.try_acquire(8).unwrap();
        permit.shrink_to(3);
        assert_eq!(budget.used(), 3);
        drop(permit);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn oversized_requests_never_wait_forever() {
        let budget = ByteBudget::new(4);
        assert!(budget.acquire(5).is_none());
    }

    #[test]
    fn closing_wakes_blocked_acquirers() {
        let budget = ByteBudget::new(1);
        let held = budget.try_acquire(1).unwrap();
        let waiter = {
            let budget = budget.clone();
            std::thread::spawn(move || budget.acquire(1).is_none())
        };
        std::thread::sleep(std::time::Duration::from_millis(20));
        budget.close();
        assert!(waiter.join().unwrap());
        drop(held);
    }

    #[test]
    fn profiles_are_ordered_and_finite() {
        let default = Limits::for_profile(Profile::Default);
        let low = Limits::for_profile(Profile::LowMemory);
        assert!(low.process_output < default.process_output);
        assert!(low.queued_requests < default.queued_requests);
        assert!(low.process_slots <= default.process_slots);
        assert_eq!(default.stream_window(), default.process_output / 2);
        assert!(default.js_input_budget() < default.process_input);
    }
}
