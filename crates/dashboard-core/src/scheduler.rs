//! Refreshing many checkouts at once with a bounded number of `gh x status` processes, reporting each result as soon
//! as it arrives.

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

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
