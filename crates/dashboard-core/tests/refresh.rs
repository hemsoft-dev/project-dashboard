//! The bounded refresh scheduler.
use dashboard_core::fixtures::{self, CLEAN};
use dashboard_core::scheduler;
use dashboard_core::source::{FetchError, StatusSource};
use dashboard_core::status::StatusDocument;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Records how many fetches overlap, and fails the ones named "bad".
#[derive(Default)]
struct Fake {
    running: AtomicUsize,
    peak: AtomicUsize,
    refreshes: Mutex<Vec<bool>>,
}

impl StatusSource for Fake {
    fn fetch(&self, checkout: &Path, refresh: bool) -> Result<StatusDocument, FetchError> {
        let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        self.refreshes.lock().unwrap().push(refresh);
        std::thread::sleep(Duration::from_millis(30));
        self.running.fetch_sub(1, Ordering::SeqCst);
        if checkout.ends_with("bad") {
            return Err(FetchError::Failed("boom".into()));
        }
        Ok(fixtures::document(CLEAN))
    }
}

#[test]
fn scheduler_runs_at_most_parallelism_at_once_and_reports_every_checkout() {
    let fake = Arc::new(Fake::default());
    let checkouts: Vec<PathBuf> = (0..10).map(|ix| PathBuf::from(format!("/src/repo-{ix}"))).collect();
    let outcomes: Vec<_> = scheduler::refresh(fake.clone(), checkouts.clone(), 4, false)
        .iter()
        .collect();
    assert_eq!(outcomes.len(), 10);
    let seen: HashSet<PathBuf> = outcomes.iter().map(|outcome| outcome.checkout.clone()).collect();
    assert_eq!(seen, checkouts.into_iter().collect());
    let peak = fake.peak.load(Ordering::SeqCst);
    assert!(peak <= 4, "peak {peak}");
    assert!(peak >= 2, "ran in parallel: peak {peak}");
}

#[test]
fn scheduler_reports_failures_without_stopping_the_others_and_passes_refresh() {
    let fake = Arc::new(Fake::default());
    let checkouts = vec![PathBuf::from("/src/bad"), PathBuf::from("/src/good")];
    let outcomes: Vec<_> = scheduler::refresh(fake.clone(), checkouts, 1, true).iter().collect();
    assert_eq!(outcomes.len(), 2);
    assert!(outcomes[0].result.is_err());
    assert!(outcomes[1].result.is_ok());
    assert_eq!(fake.peak.load(Ordering::SeqCst), 1);
    assert_eq!(*fake.refreshes.lock().unwrap(), [true, true]);
}

#[test]
fn results_arrive_as_each_finishes() {
    /// The slow checkout finishes last even though it was queued first.
    struct Staggered;
    impl StatusSource for Staggered {
        fn fetch(&self, checkout: &Path, _: bool) -> Result<StatusDocument, FetchError> {
            let delay = if checkout.ends_with("slow") { 200 } else { 10 };
            std::thread::sleep(Duration::from_millis(delay));
            Ok(fixtures::document(CLEAN))
        }
    }
    let receiver = scheduler::refresh(
        Arc::new(Staggered),
        vec![PathBuf::from("/src/slow"), PathBuf::from("/src/fast")],
        2,
        false,
    );
    assert_eq!(receiver.recv().unwrap().checkout, PathBuf::from("/src/fast"));
    assert_eq!(receiver.recv().unwrap().checkout, PathBuf::from("/src/slow"));
    assert!(receiver.recv().is_err());
}

#[test]
fn workers_never_exceed_the_checkouts_or_the_cap() {
    assert_eq!(scheduler::workers(4, 10), 4);
    assert_eq!(scheduler::workers(10_000, 2), 2);
    assert_eq!(scheduler::workers(10_000, 100), scheduler::MAX_PARALLELISM);
    assert_eq!(scheduler::workers(0, 3), 1);
    assert_eq!(scheduler::workers(4, 0), 0);
}

#[test]
fn a_huge_parallelism_still_refreshes_without_spawning_extra_threads() {
    let fake = Arc::new(Fake::default());
    let checkouts = vec![PathBuf::from("/src/a"), PathBuf::from("/src/b")];
    let outcomes: Vec<_> = scheduler::refresh(fake.clone(), checkouts, usize::MAX, false)
        .iter()
        .collect();
    assert_eq!(outcomes.len(), 2);
    assert!(fake.peak.load(Ordering::SeqCst) <= 2);
}

#[test]
fn an_empty_refresh_closes_at_once() {
    let receiver = scheduler::refresh(Arc::new(Fake::default()), Vec::new(), 4, false);
    assert!(receiver.recv().is_err());
}

#[test]
fn overlapping_refreshes_share_one_limit() {
    let fake = Arc::new(Fake::default());
    let limited: Arc<dyn StatusSource> = Arc::new(scheduler::Limited::new(fake.clone(), 1));
    // A full refresh and two single-card refreshes at once, as F5 plus card buttons would start them.
    let all = scheduler::refresh(
        limited.clone(),
        (0..4).map(|ix| PathBuf::from(format!("/src/{ix}"))).collect(),
        4,
        false,
    );
    let one = scheduler::refresh(limited.clone(), vec![PathBuf::from("/src/x")], 4, true);
    let two = scheduler::refresh(limited, vec![PathBuf::from("/src/y")], 4, true);
    let total = all.iter().count() + one.iter().count() + two.iter().count();
    assert_eq!(total, 6);
    assert_eq!(fake.peak.load(Ordering::SeqCst), 1);
}

#[test]
fn a_changed_limit_applies_to_every_refresh_that_shares_it() {
    let fake = Arc::new(Fake::default());
    let limited = Arc::new(scheduler::Limited::new(fake.clone(), 4));
    // Lowered, as a config edit from 4 to 1 would: overlapping refreshes together stay at one fetch.
    limited.set_limit(1);
    assert_eq!(limited.limit(), 1);
    let source: Arc<dyn StatusSource> = limited.clone();
    let all = scheduler::refresh(
        source.clone(),
        (0..4).map(|ix| PathBuf::from(format!("/src/{ix}"))).collect(),
        4,
        false,
    );
    let one = scheduler::refresh(source, vec![PathBuf::from("/src/x")], 4, true);
    assert_eq!(all.iter().count() + one.iter().count(), 5);
    assert_eq!(fake.peak.load(Ordering::SeqCst), 1);
    // Out-of-range limits are clamped like the config's.
    limited.set_limit(0);
    assert_eq!(limited.limit(), 1);
    limited.set_limit(10_000);
    assert_eq!(limited.limit(), scheduler::MAX_PARALLELISM);
}

#[test]
fn queued_work_for_a_removed_or_replaced_checkout_never_runs() {
    use std::collections::HashMap;
    let fake = Arc::new(Fake::default());
    let wanted = scheduler::Wanted::default();
    wanted.set([(PathBuf::from("/src/a"), 1), (PathBuf::from("/src/b"), 1)]);
    let started: HashMap<PathBuf, u64> = [(PathBuf::from("/src/a"), 1), (PathBuf::from("/src/b"), 1)].into();
    let limiter = Arc::new(scheduler::Limited::new(fake.clone(), 1));
    let gated = scheduler::OnlyWanted {
        limiter: limiter.clone(),
        wanted: wanted.clone(),
        generations: started,
    };
    // The config drops b, and a is removed and added back as a new card.
    wanted.set([(PathBuf::from("/src/a"), 2)]);
    for checkout in ["/src/a", "/src/b"] {
        let result = gated.fetch(std::path::Path::new(checkout), false);
        assert!(matches!(result, Err(FetchError::Failed(reason)) if reason == scheduler::SKIPPED));
    }
    assert!(fake.refreshes.lock().unwrap().is_empty(), "gh never ran");
    // A refresh for the new card runs.
    let current = scheduler::OnlyWanted {
        limiter: limiter.clone(),
        wanted: wanted.clone(),
        generations: [(PathBuf::from("/src/a"), 2)].into(),
    };
    assert!(current.fetch(std::path::Path::new("/src/a"), false).is_ok());
    assert_eq!(fake.refreshes.lock().unwrap().len(), 1);
    // Work that becomes unwanted while it waits for a slot is skipped once it gets one, and frees it at once.
    let became_unwanted = std::sync::atomic::AtomicBool::new(false);
    let result = limiter.fetch_if(std::path::Path::new("/src/a"), false, &|| {
        !became_unwanted.swap(true, Ordering::SeqCst)
    });
    assert!(result.is_ok(), "first check passes");
    let result = limiter.fetch_if(std::path::Path::new("/src/a"), false, &|| false);
    assert!(matches!(result, Err(FetchError::Failed(reason)) if reason == scheduler::SKIPPED));
    assert_eq!(fake.refreshes.lock().unwrap().len(), 2, "the skipped fetch never ran");
    // Its slot is free: with a limit of 1, another fetch still runs.
    assert!(limiter.fetch(std::path::Path::new("/src/a"), false).is_ok());
}
