//! Fixed-size executors with bounded, non-blocking admission.
//!
//! A task is admitted only if the queue has room for it by count and by
//! payload bytes. Admission never waits: a full queue hands the task back to
//! the caller, which answers the request with an explicit `QUEUE_FULL` error.
//! That keeps callbacks on the UI thread from ever blocking.
//!
//! Each task carries a deadline. A task that waited past it is finished with
//! [`Outcome::Expired`] instead of running, so a late request cannot start
//! work its caller has already given up on. Every task is finished exactly
//! once, with one of `Run`, `Expired`, `Rejected` or `Shutdown`, so the caller
//! can release its own bookkeeping in each case.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

/// How a task ended up being finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The task is running on a worker.
    Run,
    /// The deadline passed while the task was queued; it must not run.
    Expired,
    /// The queue was full when the task was submitted.
    Rejected,
    /// The executor closed before the task could run.
    Shutdown,
}

/// A unit of work plus the bookkeeping needed to admit it.
pub struct Task {
    bytes: usize,
    deadline: Instant,
    work: Box<dyn FnOnce(Outcome) + Send>,
}

impl Task {
    /// `bytes` is the payload retained while the task waits in the queue.
    pub fn new(
        bytes: usize,
        deadline: Instant,
        work: impl FnOnce(Outcome) + Send + 'static,
    ) -> Self {
        Self {
            bytes,
            deadline,
            work: Box::new(work),
        }
    }

    /// Finish the task with `outcome`. Consumes it, so it happens once.
    pub fn finish(self, outcome: Outcome) {
        (self.work)(outcome);
    }
}

struct Queue {
    items: VecDeque<Task>,
    bytes: usize,
    closed: bool,
    active: usize,
    shutdown_callbacks: usize,
}

#[derive(Default)]
struct Counters {
    active: AtomicUsize,
    rejected: AtomicUsize,
    expired: AtomicUsize,
    completed: AtomicUsize,
}

/// Point-in-time numbers for diagnostics. Only read by development tooling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutorStats {
    pub workers: usize,
    pub queued: usize,
    pub queued_bytes: usize,
    pub active: usize,
    pub rejected: usize,
    pub expired: usize,
    pub completed: usize,
}

pub struct Executor {
    name: &'static str,
    workers: usize,
    capacity: usize,
    byte_capacity: usize,
    queue: Arc<(Mutex<Queue>, Condvar)>,
    counters: Arc<Counters>,
}

impl Executor {
    /// Start `workers` threads that live as long as the executor.
    pub fn new(
        name: &'static str,
        workers: usize,
        capacity: usize,
        byte_capacity: usize,
    ) -> std::io::Result<Self> {
        Self::start(name, workers, capacity, byte_capacity, None)
    }

    /// Start workers with a specified stack reservation for deep workflows.
    pub fn with_stack_size(
        name: &'static str,
        workers: usize,
        capacity: usize,
        byte_capacity: usize,
        stack_size: usize,
    ) -> std::io::Result<Self> {
        Self::start(name, workers, capacity, byte_capacity, Some(stack_size))
    }

    fn start(
        name: &'static str,
        workers: usize,
        capacity: usize,
        byte_capacity: usize,
        stack_size: Option<usize>,
    ) -> std::io::Result<Self> {
        let queue = Arc::new((
            Mutex::new(Queue {
                items: VecDeque::new(),
                bytes: 0,
                closed: false,
                active: 0,
                shutdown_callbacks: 0,
            }),
            Condvar::new(),
        ));
        let counters = Arc::new(Counters::default());
        let workers = workers.max(1);
        for index in 0..workers {
            let queue = queue.clone();
            let counters = counters.clone();
            let mut builder = std::thread::Builder::new().name(format!("{name}-{index}"));
            if let Some(stack_size) = stack_size {
                builder = builder.stack_size(stack_size);
            }
            builder.spawn(move || worker(&queue, &counters))?;
        }
        Ok(Self {
            name,
            workers,
            capacity,
            byte_capacity,
            queue,
            counters,
        })
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Queue `task` without waiting. A rejected task comes back to the caller.
    pub fn submit(&self, task: Task) -> Result<(), Task> {
        let (lock, ready) = &*self.queue;
        let mut queue = lock.lock().unwrap_or_else(|e| e.into_inner());
        if queue.closed
            || queue.items.len() >= self.capacity
            || queue.bytes + task.bytes > self.byte_capacity
        {
            drop(queue);
            self.counters.rejected.fetch_add(1, Ordering::Relaxed);
            return Err(task);
        }
        queue.bytes += task.bytes;
        queue.items.push_back(task);
        drop(queue);
        ready.notify_one();
        Ok(())
    }

    pub fn stats(&self) -> ExecutorStats {
        let queue = self.queue.0.lock().unwrap_or_else(|e| e.into_inner());
        ExecutorStats {
            workers: self.workers,
            queued: queue.items.len(),
            queued_bytes: queue.bytes,
            active: self.counters.active.load(Ordering::Relaxed),
            rejected: self.counters.rejected.load(Ordering::Relaxed),
            expired: self.counters.expired.load(Ordering::Relaxed),
            completed: self.counters.completed.load(Ordering::Relaxed),
        }
    }

    /// Refuse admissions immediately without running any task callbacks.
    /// Pending tasks remain queued until `close` finishes them as shutdown.
    pub fn stop_admission(&self) {
        let (lock, ready) = &*self.queue;
        let mut queue = lock.lock().unwrap_or_else(|e| e.into_inner());
        queue.closed = true;
        drop(queue);
        ready.notify_all();
    }

    /// Refuse new work and finish every queued task with [`Outcome::Shutdown`].
    /// Tasks already running are not interrupted.
    pub fn close(&self) {
        let drained: Vec<Task> = {
            let (lock, ready) = &*self.queue;
            let mut queue = lock.lock().unwrap_or_else(|e| e.into_inner());
            queue.closed = true;
            queue.bytes = 0;
            let items: Vec<_> = queue.items.drain(..).collect();
            queue.shutdown_callbacks += items.len();
            drop(queue);
            ready.notify_all();
            items
        };
        for task in drained {
            let _ = catch_unwind(AssertUnwindSafe(|| task.finish(Outcome::Shutdown)));
            let (lock, ready) = &*self.queue;
            let mut queue = lock.lock().unwrap_or_else(|e| e.into_inner());
            queue.shutdown_callbacks -= 1;
            drop(queue);
            ready.notify_all();
        }
    }

    /// Wait until every active task and drained shutdown callback has returned.
    /// The executor must already be closed; the deadline is shared by callers
    /// waiting for multiple executors. A timeout never implies quiescence.
    pub fn wait_for_idle(&self, deadline: Instant) -> bool {
        let (lock, ready) = &*self.queue;
        let mut queue = lock.lock().unwrap_or_else(|e| e.into_inner());
        if !queue.closed {
            return false;
        }
        while queue.active != 0 || queue.shutdown_callbacks != 0 || !queue.items.is_empty() {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            let (guard, timeout) = ready
                .wait_timeout(queue, remaining)
                .unwrap_or_else(|e| e.into_inner());
            queue = guard;
            if timeout.timed_out()
                && (queue.active != 0 || queue.shutdown_callbacks != 0 || !queue.items.is_empty())
            {
                return false;
            }
        }
        true
    }
}

fn worker(queue: &(Mutex<Queue>, Condvar), counters: &Counters) {
    loop {
        let task = {
            let (lock, ready) = queue;
            let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if guard.closed {
                    break None;
                }
                if let Some(task) = guard.items.pop_front() {
                    guard.bytes -= task.bytes;
                    guard.active += 1;
                    counters.active.fetch_add(1, Ordering::Relaxed);
                    break Some(task);
                }
                guard = ready.wait(guard).unwrap_or_else(|e| e.into_inner());
            }
        };
        let Some(task) = task else { return };

        let outcome = if Instant::now() > task.deadline {
            counters.expired.fetch_add(1, Ordering::Relaxed);
            Outcome::Expired
        } else {
            Outcome::Run
        };
        // A panicking task must not shrink the pool: the worker count is fixed.
        let _ = catch_unwind(AssertUnwindSafe(|| task.finish(outcome)));
        let (lock, ready) = queue;
        let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        guard.active -= 1;
        counters.active.fetch_sub(1, Ordering::Relaxed);
        counters.completed.fetch_add(1, Ordering::Relaxed);
        drop(guard);
        ready.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    fn far() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }

    #[test]
    fn full_queue_rejects_without_waiting_and_returns_the_task() {
        // One worker, held busy, so every later task stays queued.
        let executor = Executor::new("test-full", 1, 2, 1024).unwrap();
        let (hold_tx, hold_rx) = mpsc::channel::<()>();
        executor
            .submit(Task::new(0, far(), move |_| {
                let _ = hold_rx.recv();
            }))
            .unwrap_or_else(|_| panic!("the first task is admitted"));
        // Let the worker take it off the queue.
        std::thread::sleep(Duration::from_millis(20));

        let (tx, rx) = mpsc::channel::<Outcome>();
        for _ in 0..2 {
            let tx = tx.clone();
            assert!(
                executor
                    .submit(Task::new(0, far(), move |o| tx.send(o).unwrap()))
                    .is_ok()
            );
        }
        let refused = executor.submit(Task::new(0, far(), move |o| tx.send(o).unwrap()));
        let refused = refused.expect_err("the third queued task exceeds the count limit");
        refused.finish(Outcome::Rejected);
        assert_eq!(rx.recv().unwrap(), Outcome::Rejected);

        hold_tx.send(()).unwrap();
        assert_eq!(rx.recv().unwrap(), Outcome::Run);
        assert_eq!(rx.recv().unwrap(), Outcome::Run);
        assert_eq!(executor.stats().rejected, 1);
    }

    #[test]
    fn byte_limit_is_enforced_too() {
        let executor = Executor::new("test-bytes", 1, 100, 10).unwrap();
        let (hold_tx, hold_rx) = mpsc::channel::<()>();
        executor
            .submit(Task::new(0, far(), move |_| {
                let _ = hold_rx.recv();
            }))
            .ok();
        std::thread::sleep(Duration::from_millis(20));
        assert!(executor.submit(Task::new(8, far(), |_| {})).is_ok());
        assert!(
            executor.submit(Task::new(3, far(), |_| {})).is_err(),
            "8 + 3 > 10"
        );
        hold_tx.send(()).unwrap();
    }

    #[test]
    fn expired_tasks_never_run() {
        let executor = Executor::new("test-expired", 1, 8, 1024).unwrap();
        let (hold_tx, hold_rx) = mpsc::channel::<()>();
        executor
            .submit(Task::new(0, far(), move |_| {
                let _ = hold_rx.recv();
            }))
            .ok();
        let (tx, rx) = mpsc::channel::<Outcome>();
        let past = Instant::now() - Duration::from_millis(1);
        executor
            .submit(Task::new(0, past, move |o| tx.send(o).unwrap()))
            .ok();
        std::thread::sleep(Duration::from_millis(20));
        hold_tx.send(()).unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Outcome::Expired
        );
    }

    #[test]
    fn a_panicking_task_keeps_the_worker_alive() {
        let executor = Executor::new("test-panic", 1, 8, 1024).unwrap();
        executor
            .submit(Task::new(0, far(), |_| panic!("boom")))
            .ok();
        let (tx, rx) = mpsc::channel::<Outcome>();
        executor
            .submit(Task::new(0, far(), move |o| tx.send(o).unwrap()))
            .ok();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Outcome::Run
        );
    }

    #[test]
    fn close_finishes_queued_tasks_as_shutdown() {
        let executor = Executor::new("test-close", 1, 8, 1024).unwrap();
        let (hold_tx, hold_rx) = mpsc::channel::<()>();
        executor
            .submit(Task::new(0, far(), move |_| {
                let _ = hold_rx.recv();
            }))
            .ok();
        std::thread::sleep(Duration::from_millis(20));
        let (tx, rx) = mpsc::channel::<Outcome>();
        executor
            .submit(Task::new(0, far(), move |o| tx.send(o).unwrap()))
            .ok();
        executor.close();
        assert_eq!(rx.recv().unwrap(), Outcome::Shutdown);
        assert!(executor.submit(Task::new(0, far(), |_| {})).is_err());
        hold_tx.send(()).unwrap();
    }

    #[test]
    fn idle_wait_never_finishes_before_a_blocked_writer() {
        let root = std::env::temp_dir().join(format!(
            "executor-writer-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let executor = Arc::new(Executor::new("test-writer", 1, 4, 1024).unwrap());
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let writer_root = root.clone();
        executor
            .submit(Task::new(0, far(), move |outcome| {
                assert_eq!(outcome, Outcome::Run);
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                std::fs::create_dir_all(&writer_root).unwrap();
                std::fs::write(writer_root.join("state"), "written").unwrap();
            }))
            .ok()
            .unwrap();
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        executor.close();
        assert!(!executor.wait_for_idle(Instant::now() + Duration::from_millis(20)));
        let waiter = executor.clone();
        let cleanup_root = root.clone();
        let (cleaned_tx, cleaned_rx) = mpsc::channel();
        std::thread::spawn(move || {
            assert!(waiter.wait_for_idle(Instant::now() + Duration::from_secs(5)));
            std::fs::remove_dir_all(cleanup_root).unwrap();
            cleaned_tx.send(()).unwrap();
        });
        assert!(cleaned_rx.recv_timeout(Duration::from_millis(20)).is_err());
        release_tx.send(()).unwrap();
        cleaned_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            !root.exists(),
            "the writer cannot recreate data after cleanup"
        );
    }

    #[test]
    fn idle_wait_includes_shutdown_callbacks_even_during_concurrent_close() {
        let executor = Arc::new(Executor::new("test-close-callback", 1, 4, 1024).unwrap());
        let (started_tx, started_rx) = mpsc::channel();
        let (worker_tx, worker_rx) = mpsc::channel::<()>();
        executor
            .submit(Task::new(0, far(), move |_| {
                started_tx.send(()).unwrap();
                worker_rx.recv().unwrap();
            }))
            .ok()
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (closing_tx, closing_rx) = mpsc::channel();
        let (callback_tx, callback_rx) = mpsc::channel::<()>();
        executor
            .submit(Task::new(0, far(), move |outcome| {
                assert_eq!(outcome, Outcome::Shutdown);
                closing_tx.send(()).unwrap();
                callback_rx.recv().unwrap();
            }))
            .ok()
            .unwrap();
        let closer = executor.clone();
        let thread = std::thread::spawn(move || closer.close());
        closing_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        worker_tx.send(()).unwrap();
        assert!(!executor.wait_for_idle(Instant::now() + Duration::from_millis(20)));
        callback_tx.send(()).unwrap();
        thread.join().unwrap();
        assert!(executor.wait_for_idle(Instant::now() + Duration::from_secs(5)));
    }

    #[test]
    fn stopping_admission_never_starts_queued_work_before_close() {
        let executor = Executor::new("test-stop-admission", 1, 4, 1024).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        executor
            .submit(Task::new(0, far(), move |_| {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            }))
            .ok()
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (outcome_tx, outcome_rx) = mpsc::channel();
        executor
            .submit(Task::new(0, far(), move |outcome| {
                outcome_tx.send(outcome).unwrap();
            }))
            .ok()
            .unwrap();
        executor.stop_admission();
        assert!(executor.submit(Task::new(0, far(), |_| {})).is_err());
        release_tx.send(()).unwrap();
        assert!(!executor.wait_for_idle(Instant::now() + Duration::from_millis(20)));
        assert!(outcome_rx.try_recv().is_err());
        executor.close();
        assert_eq!(
            outcome_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Outcome::Shutdown
        );
        assert!(executor.wait_for_idle(Instant::now() + Duration::from_secs(5)));
    }
}
