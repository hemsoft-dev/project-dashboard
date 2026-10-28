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

fn hosted_good() -> StatusDocument {
    let mut doc = good();
    doc.verdict = None;
    doc.hosted = Some(
        serde_json::from_value(serde_json::json!({
            "serviceUrl":"http://pmd.test", "generatedAt":"2026-10-25T20:00:00Z",
            "lastError":null, "board":{"generatedAt":"2026-10-25T20:00:00Z","columns":[]},
            "work":[], "workError":null
        }))
        .unwrap(),
    );
    doc
}

#[test]
fn retained_upstream_failure_stays_failed_when_transport_is_offline() {
    let path = PathBuf::from("/fixture");
    let mut doc = hosted_good();
    doc.hosted.as_mut().unwrap().last_error = Some(dashboard_core::pmd::LastError {
        message: "upstream GitHub refresh failed".into(),
        at: "2026-10-25T20:00:00Z".into(),
    });
    let mut cache = StatusCache::default();
    cache.insert(path.clone(), doc.clone());
    let mut board = Board::new(std::slice::from_ref(&path), &cache);
    for error in [FetchError::Timeout, FetchError::Connect] {
        board.apply(
            Outcome {
                checkout: path.clone(),
                result: Err(error),
            },
            &mut cache,
        );
        assert_eq!(board.cards()[0].overall(), Overall::Failed);
        assert_eq!(board.cards()[0].document, Some(doc.clone()));
        assert!(board.cards()[0].recovery.retry_at.is_some());
    }
}

#[test]
fn partial_transport_failures_back_off_until_the_whole_read_succeeds() {
    use dashboard_core::source::TransportError;
    let mut recovery = Recovery::default();
    let mut now = Instant::now();
    recovery.record(&Ok(hosted_good()), now, chrono::Local::now());
    let last_success = recovery.last_success;
    for (transport, seconds) in [
        (TransportError::Timeout, 5),
        (TransportError::Connect, 10),
        (TransportError::Timeout, 20),
        (TransportError::Connect, 60),
    ] {
        let mut partial = hosted_good();
        let hosted = partial.hosted.as_mut().unwrap();
        hosted.work_transport_error = Some(transport);
        hosted.work_error = Some("transport failed".into());
        recovery.record(&Ok(partial), now, chrono::Local::now());
        assert_eq!(recovery.last_success, last_success);
        assert_eq!(recovery.retry_at, Some(now + Duration::from_secs(seconds)));
        now += Duration::from_secs(seconds);
        assert!(recovery.due(now));
    }
    recovery.record(&Ok(hosted_good()), now, chrono::Local::now());
    assert_eq!(recovery.retry_at, None);
    let mut http = hosted_good();
    http.hosted.as_mut().unwrap().work_error = Some(FetchError::Http(503).to_string());
    recovery.record(&Ok(http), now, chrono::Local::now());
    assert_eq!(recovery.retry_at, None);
}
