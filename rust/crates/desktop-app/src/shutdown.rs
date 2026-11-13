//! Run shutdown away from the UI thread, with an independent timeout.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

/// A modal action admitted before shutdown must not block the event loop while
/// its worker waits for a reply. Reject it and release that worker instead.
pub fn reject_window_action(closing: bool, reply: &mut Option<crate::host::ActionReply>) -> bool {
    if closing {
        if let Some(reply) = reply.take() {
            let _ = reply.send(Err(desktop_core::error::AppError::msg(
                "application is shutting down",
            )));
        }
        true
    } else {
        false
    }
}

/// `work` receives the shared deadline and returns true only if it observed all
/// tracked work finish. The notification is emitted once, even if a stuck
/// callback outlives the deadline. A false result never authorizes cleanup.
pub fn start(
    timeout: Duration,
    work: impl FnOnce(Instant) -> bool + Send + 'static,
    notify: impl Fn(bool) + Send + Sync + 'static,
) -> std::io::Result<()> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .unwrap_or_else(Instant::now);
    let finished = Arc::new(AtomicBool::new(false));
    let notify = Arc::new(notify);
    let timer_finished = finished.clone();
    let timer_notify = notify.clone();
    std::thread::Builder::new()
        .name("desktop-shutdown-timer".into())
        .spawn(move || {
            std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
            if !timer_finished.swap(true, Ordering::AcqRel) {
                timer_notify(false);
            }
        })?;
    let worker_finished = finished.clone();
    let worker_notify = notify.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("desktop-shutdown".into())
        .spawn(move || {
            let drained = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(deadline)))
                .unwrap_or(false)
                && Instant::now() <= deadline;
            if !worker_finished.swap(true, Ordering::AcqRel) {
                worker_notify(drained);
            }
        })
    {
        finished.store(true, Ordering::Release);
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn closing_rejects_modal_action_and_releases_waiting_worker() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut reply = Some(sender);
        assert!(!reject_window_action(false, &mut reply));
        assert!(reject_window_action(true, &mut reply));
        assert!(
            receiver
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .is_err()
        );
        assert!(reply.is_none());
    }
    #[test]
    fn timeout_notifies_without_waiting_for_a_stuck_callback() {
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (notify_tx, notify_rx) = mpsc::channel();
        start(
            Duration::from_millis(100),
            move |_| {
                release_rx.recv().unwrap();
                true
            },
            move |value| {
                notify_tx.send(value).unwrap();
            },
        )
        .unwrap();
        assert!(!notify_rx.recv_timeout(Duration::from_secs(5)).unwrap());
        release_tx.send(()).unwrap();
        assert!(notify_rx.recv_timeout(Duration::from_millis(150)).is_err());
    }
    #[test]
    fn finished_work_reports_once_and_does_not_block_the_caller() {
        let (request_tx, request_rx) = mpsc::channel();
        let (response_tx, response_rx) = mpsc::channel::<()>();
        let (notify_tx, notify_rx) = mpsc::channel();
        start(
            Duration::from_secs(2),
            move |_| {
                request_tx.send(()).unwrap();
                response_rx.recv().unwrap();
                true
            },
            move |value| {
                notify_tx.send(value).unwrap();
            },
        )
        .unwrap();
        request_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        response_tx.send(()).unwrap();
        assert!(notify_rx.recv_timeout(Duration::from_secs(1)).unwrap());
    }
}
