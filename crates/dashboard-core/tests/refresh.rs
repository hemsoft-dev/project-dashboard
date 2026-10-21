//! Running gh x status: interpreting its output, and the bounded parallel scheduler.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dashboard_core::fixtures::{self, CLEAN, ERROR};
use dashboard_core::scheduler;
use dashboard_core::source::{FetchError, GhX, StatusSource, interpret};
use dashboard_core::status::StatusDocument;

#[test]
fn gh_x_runs_status_json_and_adds_refresh_only_when_asked() {
    assert_eq!(GhX::args(false), ["x", "status", "--json"]);
    assert_eq!(GhX::args(true), ["x", "status", "--json", "--refresh"]);
}

#[test]
fn a_document_on_stdout_is_read_whatever_the_exit_code() {
    assert_eq!(interpret(CLEAN.as_bytes(), b"", true), Ok(fixtures::document(CLEAN)));
    assert_eq!(
        interpret(ERROR.as_bytes(), b"", false),
        Err(FetchError::Failed(
            "not a git repository (or any of the parent directories): .git".into()
        ))
    );
}

#[test]
fn a_missing_extension_or_flag_is_a_setup_problem() {
    let missing = interpret(b"", b"unknown command \"x\" for \"gh\"\n\nDid you mean this?", false).unwrap_err();
    assert!(matches!(missing, FetchError::GhXMissing(_)));
    assert!(missing.is_setup());
    let old = interpret(b"", b"unknown flag: --json\nUsage: gh x status", false).unwrap_err();
    assert!(matches!(old, FetchError::GhXMissing(_)));
    assert!(old.to_string().contains("gh extension upgrade gh-x"));
    // Not every mention of an unknown command is gh-x missing.
    let other = interpret(b"", b"unknown command \"frobnicate\" for \"gh x\"", false).unwrap_err();
    assert!(!other.is_setup());
}

/// The real stderr of gh-x v0.21.2, which predates --json (captured 2026-10-21 on mini).
const GH_X_0_21_2_NO_JSON: &str = include_str!("fixtures/gh-x-0.21.2-no-json.stderr.txt");

#[test]
fn a_gh_x_without_json_is_a_setup_problem() {
    let err = interpret(b"", GH_X_0_21_2_NO_JSON.as_bytes(), false).unwrap_err();
    assert_eq!(
        err,
        FetchError::GhXMissing("flag provided but not defined: -json".into())
    );
}

#[test]
fn failures_skip_the_gh_x_banner() {
    assert_eq!(
        interpret(
            b"",
            b"gh-x v0.21.2 (2026-10-19) \xC2\xA9 2026 HemSoft Developments\nnot a git repository\n",
            false
        ),
        Err(FetchError::Failed("not a git repository".into()))
    );
}

#[test]
fn other_failures_keep_gh_first_line() {
    assert_eq!(
        interpret(b"", b"HTTP 401: Bad credentials\nTry gh auth login", false),
        Err(FetchError::Failed("HTTP 401: Bad credentials".into()))
    );
    assert_eq!(
        interpret(b"", b"", false),
        Err(FetchError::Failed("gh x status failed without a message".into()))
    );
    assert!(matches!(interpret(b"not json", b"", true), Err(FetchError::Failed(_))));
}

#[test]
fn a_missing_gh_program_is_reported_as_gh_missing() {
    let source = GhX {
        program: "gh-that-does-not-exist-anywhere".into(),
        ..GhX::default()
    };
    let here = std::env::current_dir().unwrap();
    assert_eq!(source.fetch(&here, false), Err(FetchError::GhMissing));
    assert_eq!(
        source.fetch(Path::new("/definitely/not/a/folder"), false),
        Err(FetchError::MissingFolder)
    );
}

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

/// A stand-in `gh`: a shell script that ignores its arguments.
#[cfg(unix)]
fn fake_gh(name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = std::env::temp_dir().join(format!("project-dashboard-fake-gh-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("gh");
    std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

#[cfg(unix)]
#[test]
fn a_stalled_gh_is_stopped_at_the_deadline() {
    let source = GhX {
        program: fake_gh("stall", "sleep 30").into(),
        timeout: Duration::from_millis(300),
    };
    let started = std::time::Instant::now();
    let result = source.fetch(&std::env::temp_dir(), false);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "returned after {:?}",
        started.elapsed()
    );
    let err = result.unwrap_err();
    assert!(err.to_string().contains("didn't finish within"), "{err}");
}

#[cfg(unix)]
#[test]
fn a_real_process_output_is_read_through_the_pipes() {
    let fixture = std::env::temp_dir().join(format!("project-dashboard-fixture-{}.json", std::process::id()));
    std::fs::write(&fixture, CLEAN).unwrap();
    let source = GhX {
        program: fake_gh("ok", &format!("cat '{}'", fixture.display())).into(),
        timeout: Duration::from_secs(30),
    };
    assert_eq!(source.fetch(&std::env::temp_dir(), true), Ok(fixtures::document(CLEAN)));
    let failing = GhX {
        program: fake_gh("fail", "echo 'HTTP 401: Bad credentials' >&2; exit 1").into(),
        timeout: Duration::from_secs(30),
    };
    assert_eq!(
        failing.fetch(&std::env::temp_dir(), false),
        Err(FetchError::Failed("HTTP 401: Bad credentials".into()))
    );
}
