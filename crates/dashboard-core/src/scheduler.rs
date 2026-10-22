//! Refreshing many checkouts at once with a bounded number of `gh x status` processes, reporting each result as soon
//! as it arrives.

use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};

use crate::source::{FetchError, StatusSource};
use crate::status::StatusDocument;

/// The most `gh x status` processes that ever run at once.
pub const MAX_PARALLELISM: usize = 16;

/// How many workers a refresh of `checkouts` checkouts starts for a configured `parallelism`.
pub fn workers(parallelism: usize, checkouts: usize) -> usize {
    parallelism.clamp(1, MAX_PARALLELISM).min(checkouts)
}

/// One checkout's refresh result.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub checkout: PathBuf,
    pub result: Result<StatusDocument, FetchError>,
}

/// Fetches every checkout with at most `parallelism` running at once, sending each outcome on the returned channel as
/// it completes. The channel closes once all are done. Runs on its own threads, so the caller never blocks.
pub fn refresh(
    source: Arc<dyn StatusSource>,
    checkouts: Vec<PathBuf>,
    parallelism: usize,
    refresh: bool,
) -> mpsc::Receiver<Outcome> {
    let (sender, receiver) = mpsc::channel();
    // Never more threads than checkouts, nor than the hard cap, whatever the config says.
    let workers = workers(parallelism, checkouts.len());
    let queue = Arc::new(Mutex::new(checkouts.into_iter()));
    for _ in 0..workers {
        let queue = Arc::clone(&queue);
        let source = Arc::clone(&source);
        let sender = sender.clone();
        std::thread::spawn(move || {
            loop {
                let next = queue.lock().map(|mut queue| queue.next()).unwrap_or(None);
                let Some(checkout) = next else { break };
                let result = source.fetch(&checkout, refresh);
                if sender.send(Outcome { checkout, result }).is_err() {
                    // Nobody is listening any more; stop starting new processes.
                    break;
                }
            }
        });
    }
    receiver
}

/// A source that lets at most `limit` fetches run at once, across every refresh that shares it. The board wraps its
/// source in one, so a per-card refresh started during F5 or the timer still counts against `parallelism`. The limit
/// can change while fetches run (#21): it is one counter for everything, so lowering it never lets old and new work
/// add up past the new limit; fetches already running finish, and no new one starts until the count is under it.
pub struct Limited {
    inner: Arc<dyn StatusSource>,
    limit: AtomicUsize,
    running: Mutex<usize>,
    freed: Condvar,
}

impl Limited {
    pub fn new(inner: Arc<dyn StatusSource>, limit: usize) -> Self {
        Self {
            inner,
            limit: AtomicUsize::new(limit.clamp(1, MAX_PARALLELISM)),
            running: Mutex::new(0),
            freed: Condvar::new(),
        }
    }

    pub fn limit(&self) -> usize {
        self.limit.load(Ordering::SeqCst)
    }

    /// Changes the limit for fetches that start from now on, waking any that a higher limit lets through.
    pub fn set_limit(&self, limit: usize) {
        // Taken under the lock, so a fetch can't miss the wake-up between checking the limit and waiting.
        let _running = self.running.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.limit.store(limit.clamp(1, MAX_PARALLELISM), Ordering::SeqCst);
        self.freed.notify_all();
    }
}

impl StatusSource for Limited {
    fn fetch(&self, checkout: &Path, refresh: bool) -> Result<StatusDocument, FetchError> {
        {
            let mut running = self.running.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            while *running >= self.limit() {
                running = self
                    .freed
                    .wait(running)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            *running += 1;
        }
        let result = self.inner.fetch(checkout, refresh);
        *self.running.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) -= 1;
        self.freed.notify_one();
        result
    }
}
