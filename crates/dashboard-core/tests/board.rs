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
fn a_missing_gh_is_one_setup_problem() {
    let mut cache = StatusCache::default();
    let mut board = Board::new(&paths(), &cache);
    for checkout in paths() {
        board.apply(
            Outcome {
                checkout,
                result: Err(FetchError::GhMissing),
            },
            &mut cache,
        );
    }
    assert_eq!(board.setup_problem(), Some(&FetchError::GhMissing));
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
