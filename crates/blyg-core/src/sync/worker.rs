//! The background sync thread: sleeps until the next op is due (debounce),
//! the next pull is due, or it's woken; respects the offline backoff.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::Engine;
use crate::api::is_transient;
use crate::util::now_ms;

pub(crate) enum Msg {
    /// Something was queued; recompute the next deadline.
    Wake,
    /// Pull at the next opportunity.
    Pull,
    Shutdown,
}

/// Never spin faster than this, whatever the deadlines say.
const MIN_WAIT: Duration = Duration::from_millis(10);

pub(crate) fn spawn(engine: Arc<Engine>, rx: Receiver<Msg>) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("blyg-sync".into())
        .spawn(move || run(engine, rx))
        .expect("spawn sync worker")
}

fn run(engine: Arc<Engine>, rx: Receiver<Msg>) {
    let mut next_pull = Instant::now();
    loop {
        let now = Instant::now();
        let mut deadline = next_pull;
        if let Some(due) = engine.store.earliest_due() {
            let d = now + Duration::from_millis((due - now_ms()).max(0) as u64);
            deadline = deadline.min(d);
        }
        if let Some(retry) = engine.retry_at() {
            deadline = deadline.max(retry);
        }
        let wait = deadline.saturating_duration_since(now).max(MIN_WAIT);
        match rx.recv_timeout(wait) {
            Ok(Msg::Wake) => continue,
            Ok(Msg::Pull) => {
                next_pull = Instant::now();
                continue;
            }
            Ok(Msg::Shutdown) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if engine.retry_at().is_some_and(|r| Instant::now() < r) {
            continue;
        }
        let flushed = engine.flush(None, false);
        if matches!(&flushed, Err(e) if is_transient(e)) {
            continue; // don't pull into a dead network; backoff decides when to retry
        }
        if Instant::now() >= next_pull {
            next_pull = match engine.pull() {
                // Offline: try again when the backoff expires, not a full interval later.
                Err(e) if is_transient(&e) => engine.retry_at().unwrap_or_else(Instant::now),
                _ => Instant::now() + engine.opts.pull_interval,
            };
        }
    }
}
