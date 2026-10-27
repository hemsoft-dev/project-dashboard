//! The board: cached cards first, refresh outcomes applied as they arrive, failures kept per card.

use std::path::PathBuf;

use dashboard_core::cache::StatusCache;
use dashboard_core::fixtures::{self, ATTENTION, CLEAN};
use dashboard_core::scheduler::Outcome;
use dashboard_core::source::FetchError;
use dashboard_core::{Board, EmptyState, Overall};

fn paths() -> Vec<PathBuf> {
    vec!["/src/gh-x".into(), "/src/codexbar".into()]
}

#[test]
fn empty_board_shows_the_no_repositories_message() {
    let board = Board::new(&[], &StatusCache::default());
    assert!(board.is_empty());
    assert_eq!(board.empty_state(), Some(EmptyState::NO_REPOSITORIES));
}

#[test]
fn cards_start_from_the_cache_and_load_the_rest() {
    let mut cache = StatusCache::default();
    cache.insert("/src/gh-x".into(), fixtures::document(CLEAN));
    let board = Board::new(&paths(), &cache);
    assert_eq!(board.empty_state(), None);
    let [cached, unknown] = board.cards() else { panic!() };
    assert!(cached.from_cache);
    assert_eq!(cached.overall(), Overall::Clean);
    assert_eq!(cached.name(), "hemsoft-dev/gh-x");
    assert_eq!(unknown.overall(), Overall::Loading);
    assert_eq!(unknown.name(), "codexbar");
}

#[test]
fn outcomes_update_their_card_and_the_cache() {
    let mut cache = StatusCache::default();
    let mut board = Board::new(&paths(), &cache);
    board.start_refresh(&paths());
    assert!(board.is_refreshing());
    let applied = board.apply(
        Outcome {
            checkout: "/src/codexbar".into(),
            result: Ok(fixtures::document(ATTENTION)),
        },
        &mut cache,
    );
    assert!(applied);
    let card = board.card("/src/codexbar".as_ref()).unwrap();
    assert!(!card.refreshing && !card.from_cache);
    assert_eq!(card.overall(), Overall::Attention);
    assert!(cache.get("/src/codexbar".as_ref()).is_some());
    // The other card is still refreshing.
    assert!(board.is_refreshing());
}

#[test]
fn a_failure_keeps_the_last_good_status_and_marks_only_its_card() {
    let mut cache = StatusCache::default();
    cache.insert("/src/gh-x".into(), fixtures::document(CLEAN));
    let mut board = Board::new(&paths(), &cache);
    board.apply(
        Outcome {
            checkout: "/src/gh-x".into(),
            result: Err(FetchError::Failed("HTTP 401: Bad credentials".into())),
        },
        &mut cache,
    );
    let failed = board.card("/src/gh-x".as_ref()).unwrap();
    assert_eq!(failed.overall(), Overall::Failed);
    assert!(failed.document.is_some());
    assert_eq!(
        board.card("/src/codexbar".as_ref()).unwrap().overall(),
        Overall::Loading
    );
    assert_eq!(board.setup_problem(), None);
}

#[test]
fn a_missing_pmd_token_is_one_setup_problem() {
    let mut cache = StatusCache::default();
    let mut board = Board::new(&paths(), &cache);
    for checkout in paths() {
        board.apply(
            Outcome {
                checkout,
                result: Err(FetchError::MissingToken),
            },
            &mut cache,
        );
    }
    assert_eq!(board.setup_problem(), Some(&FetchError::MissingToken));
}

#[test]
fn outcomes_for_removed_checkouts_are_ignored() {
    let mut cache = StatusCache::default();
    let mut board = Board::new(&paths(), &cache);
    let applied = board.apply(
        Outcome {
            checkout: "/src/gone".into(),
            result: Ok(fixtures::document(CLEAN)),
        },
        &mut cache,
    );
    assert!(!applied);
    assert!(cache.is_empty());
}

#[test]
fn a_checkout_listed_twice_gets_one_card_and_finishes_refreshing() {
    let mut cache = StatusCache::default();
    let listed = vec![
        PathBuf::from("/src/gh-x"),
        PathBuf::from("/src/codexbar"),
        PathBuf::from("/src/gh-x"),
    ];
    let mut board = Board::new(&listed, &cache);
    assert_eq!(board.checkouts(), paths());
    board.start_refresh(&board.checkouts());
    for checkout in board.checkouts() {
        board.apply(
            Outcome {
                checkout,
                result: Ok(fixtures::document(CLEAN)),
            },
            &mut cache,
        );
    }
    assert!(!board.is_refreshing());
}

#[test]
fn an_edited_list_keeps_known_cards_adds_new_ones_and_drops_the_rest() {
    let mut cache = StatusCache::default();
    let mut board = Board::new(&paths(), &cache);
    board.apply(
        Outcome {
            checkout: "/src/gh-x".into(),
            result: Ok(fixtures::document(CLEAN)),
        },
        &mut cache,
    );
    // minibot has a cached status from an earlier run; weather-cli has nothing yet.
    cache.insert("/src/minibot".into(), fixtures::document(ATTENTION));
    let added = board.set_checkouts(
        &[
            "/src/minibot".into(),
            "/src/gh-x".into(),
            "/src/weather-cli".into(),
            "/src/minibot".into(),
        ],
        &cache,
    );
    assert_eq!(
        added,
        [PathBuf::from("/src/minibot"), PathBuf::from("/src/weather-cli")]
    );
    // The new order, codexbar gone, a repeated checkout once.
    assert_eq!(
        board.checkouts(),
        [
            PathBuf::from("/src/minibot"),
            PathBuf::from("/src/gh-x"),
            PathBuf::from("/src/weather-cli")
        ]
    );
    let [minibot, gh_x, weather] = board.cards() else {
        panic!()
    };
    // gh-x keeps its fresh status; minibot starts from the cache, weather-cli from nothing.
    assert_eq!(gh_x.overall(), Overall::Clean);
    assert!(!gh_x.from_cache);
    assert!(minibot.from_cache);
    assert_eq!(weather.overall(), Overall::Loading);
    // The same list again changes nothing and adds nothing.
    let before = board.clone();
    assert!(board.set_checkouts(&board.checkouts(), &cache).is_empty());
    assert_eq!(board, before);
}

#[test]
fn a_refresh_for_a_removed_card_does_not_land_on_its_replacement() {
    let mut cache = StatusCache::default();
    let mut board = Board::new(&paths(), &cache);
    // A refresh starts for gh-x, then gh-x is removed and added back before it finishes.
    let old = board.start_refresh(&[PathBuf::from("/src/gh-x")]);
    board.set_checkouts(&["/src/codexbar".into()], &cache);
    board.set_checkouts(&paths(), &cache);
    let new = board.start_refresh(&[PathBuf::from("/src/gh-x")]);
    assert_ne!(
        old["/src/gh-x".as_ref() as &std::path::Path],
        new[std::path::Path::new("/src/gh-x")]
    );
    let outcome = || Outcome {
        checkout: "/src/gh-x".into(),
        result: Ok(fixtures::document(CLEAN)),
    };
    // The old refresh's result is ignored: the new card is still refreshing and has no status.
    assert!(!board.apply_started(outcome(), old[std::path::Path::new("/src/gh-x")], &mut cache));
    let card = board.card(std::path::Path::new("/src/gh-x")).unwrap();
    assert!(card.refreshing);
    assert!(card.document.is_none());
    // Its own refresh lands.
    assert!(board.apply_started(outcome(), new[std::path::Path::new("/src/gh-x")], &mut cache));
    assert_eq!(
        board.card(std::path::Path::new("/src/gh-x")).unwrap().overall(),
        Overall::Clean
    );
}

#[test]
fn hosted_signals_order_blocked_then_waiting_then_failing_checks() {
    use dashboard_core::Signals;
    let signals = |blocked, waiting_on_franz, failing: &[&str], board| Signals {
        blocked,
        waiting_on_franz,
        failing: failing.iter().map(|text| text.to_string()).collect(),
        board,
    };
    assert_eq!(signals(1, 2, &["PR #12"], true).overall(), Overall::Blocked);
    assert_eq!(signals(0, 2, &["PR #12"], true).overall(), Overall::WaitingOnFranz);
    assert_eq!(signals(0, 0, &["PR #12"], true).overall(), Overall::ChecksFailing);
    assert_eq!(signals(0, 0, &["PR #12"], false).overall(), Overall::ChecksFailing);
    assert_eq!(signals(0, 0, &[], false).overall(), Overall::Unavailable);
    assert_eq!(signals(0, 0, &[], true).overall(), Overall::Clean);
    assert_eq!(
        signals(1, 2, &["PR #12", "CI on main"], false).describe(),
        [
            "Blocked: 1",
            "Waiting on Franz: 2",
            "Checks failing: PR #12, CI on main",
            "Board unavailable"
        ]
    );
}

#[test]
fn failing_checks_are_open_prs_that_fail_and_the_default_branchs_newest_failed_runs() {
    let mut document = fixtures::document(ATTENTION);
    // The attention fixture: PR #232's checks fail; its failed run is on a feature branch, so it doesn't count.
    assert_eq!(document.failing_checks(), ["PR #232"]);
    let mut failed = document.workflow_runs[0].clone();
    failed.branch = "main".into();
    failed.started_at = Some("2026-10-21T06:29:00Z".into());
    let mut fixed = failed.clone();
    fixed.status = "✓".into();
    fixed.started_at = Some("2026-10-21T06:29:30Z".into());
    document.workflow_runs.push(failed);
    assert_eq!(document.failing_checks(), ["PR #232", "CI on main"]);
    // A newer passing run of the same workflow on main clears it.
    document.workflow_runs.push(fixed);
    assert_eq!(document.failing_checks(), ["PR #232"]);
    assert!(fixtures::document(CLEAN).failing_checks().is_empty());
}
