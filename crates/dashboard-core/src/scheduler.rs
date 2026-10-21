//! Refreshing many checkouts at once with a bounded number of `gh x status` processes, reporting each result as soon
//! as it arrives.

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use crate::source::{FetchError, StatusSource};
use crate::status::StatusDocument;

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
    let queue = Arc::new(Mutex::new(checkouts.into_iter()));
    let workers = parallelism.max(1);
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
