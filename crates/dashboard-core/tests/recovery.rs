use dashboard_core::{
    Board, Card, Overall, cache::StatusCache, fixtures, recovery::Recovery, scheduler::Outcome, source::FetchError,
    status::StatusDocument,
};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn good() -> StatusDocument {
    StatusDocument::parse(fixtures::CLEAN).unwrap()
}

#[test]
fn transport_backoff_caps_and_success_resets_it() {
    let mut recovery = Recovery::default();
    let mut now = Instant::now();
    for seconds in [5, 10, 20, 60, 60, 60] {
        recovery.record(&Err(FetchError::Timeout), now, chrono::Local::now());
        assert!(!recovery.due(now + Duration::from_secs(seconds - 1)));
        now += Duration::from_secs(seconds);
        assert!(recovery.due(now));
    }
    recovery.record(&Ok(good()), now, chrono::Local::now());
    assert!(recovery.last_success.is_some());
    assert!(!recovery.due(now + Duration::from_secs(600)));
    recovery.record(&Err(FetchError::Connect), now, chrono::Local::now());
    assert_eq!(recovery.retry_at, Some(now + Duration::from_secs(5)));
    recovery.record(&Err(FetchError::Http(503)), now, chrono::Local::now());
    assert_eq!(recovery.retry_at, None);
}

#[test]
fn only_connect_and_timeout_are_transient() {
    for error in [FetchError::Connect, FetchError::Timeout] {
        assert!(error.is_transient());
    }
    for error in [
        FetchError::MissingToken,
        FetchError::Unauthorized,
        FetchError::Http(403),
        FetchError::Http(404),
        FetchError::Http(500),
        FetchError::Unavailable,
        FetchError::Malformed,
        FetchError::Identity,
        FetchError::NoStatus,
        FetchError::Failed("failure".into()),
    ] {
        assert!(!error.is_transient());
    }
}

#[test]
fn startup_then_success_then_offline_retains_data_and_original_success_time() {
    let path = PathBuf::from("/fixture");
    let mut cache = StatusCache::default();
    let mut board = Board::new(std::slice::from_ref(&path), &cache);
    let apply = |board: &mut Board, cache: &mut StatusCache, result| {
        board.apply(
            Outcome {
                checkout: path.clone(),
                result,
            },
            cache,
        );
    };
    apply(&mut board, &mut cache, Err(FetchError::Connect));
    assert_eq!(board.cards()[0].overall(), Overall::Loading);
    assert!(board.cards()[0].recovery.last_success.is_none());
    apply(&mut board, &mut cache, Ok(good()));
    let success = board.cards()[0].clone();
    apply(&mut board, &mut cache, Err(FetchError::Timeout));
    let stale = &board.cards()[0];
    assert_eq!(stale.document, success.document);
    assert_eq!(stale.overall(), success.overall());
    assert_eq!(stale.recovery.last_success, success.recovery.last_success);
    assert!(stale.from_cache);
    apply(&mut board, &mut cache, Err(FetchError::Http(500)));
    assert_eq!(board.cards()[0].overall(), Overall::Failed);
    assert!(board.cards()[0].recovery.retry_at.is_none());
    apply(&mut board, &mut cache, Ok(good()));
    assert!(board.cards()[0].error.is_none());
    assert!(!board.cards()[0].from_cache);
    board.invalidate(&path, &mut cache);
    assert_eq!(board.cards()[0].recovery, Recovery::default());
    assert_eq!(Card::new(path).overall(), Overall::Loading);
}
