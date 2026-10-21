//! Headless UI tests: the real board view in an in-process test window, with the fixture-backed demo source. Nothing
//! here opens a desktop window, runs gh, sends OS input or changes focus outside the test.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use dashboard_core::cache::StatusCache;
use dashboard_core::fixtures::{self, ATTENTION, CLEAN, UNAVAILABLE};
use dashboard_core::source::{FetchError, StatusSource};
use dashboard_core::status::StatusDocument;
use dashboard_core::{EmptyState, Overall};
use gpui_kit::component::{ActiveTheme as _, Root};
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, px, size};

use crate::board_view::{self, BoardView, Options, counts_line};
use crate::{demo, theme};

fn options() -> Options {
    Options {
        cache_dir: None,
        parallelism: 4,
        auto_refresh: None,
        config_problem: None,
        config_path: None,
    }
}

fn open_board(
    cx: &mut TestAppContext,
    checkouts: Vec<PathBuf>,
    cache: StatusCache,
    source: Arc<dyn StatusSource>,
    options: Options,
) -> (AnyWindowHandle, Entity<BoardView>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        theme::init(cx);
        board_view::bind_keys(cx);
    });
    let mut view = None;
    let handle = cx.open_window(size(px(1100.), px(900.)), |window, cx| {
        let board = cx.new(|cx| BoardView::new(&checkouts, cache, source, options, window, cx));
        view = Some(board.clone());
        Root::new(board, window, cx)
    });
    let handle: AnyWindowHandle = handle.into();
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
    (handle, view.unwrap())
}

/// Runs the GPUI clock until every card has its result; the scheduler's real threads need real time too.
async fn settle(cx: &mut TestAppContext, handle: AnyWindowHandle, view: &Entity<BoardView>) {
    for _ in 0..200 {
        std::thread::sleep(Duration::from_millis(2));
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        if !cx.update(|cx| view.read(cx).board().is_refreshing()) {
            break;
        }
    }
    cx.wait_for(handle, Duration::from_secs(1), |_, _| true).await;
}

fn overall(cx: &mut TestAppContext, view: &Entity<BoardView>) -> Vec<Overall> {
    cx.update(|cx| {
        view.read(cx)
            .board()
            .cards()
            .iter()
            .map(|card| card.overall())
            .collect()
    })
}

#[gpui_kit::test]
fn empty_board_renders_its_empty_state(cx: &mut TestAppContext) {
    let (handle, view) = open_board(
        cx,
        vec![],
        StatusCache::default(),
        Arc::new(demo::DemoSource),
        options(),
    );
    let empty = cx.update(|cx| view.read(cx).board().empty_state());
    assert_eq!(empty, Some(EmptyState::NO_REPOSITORIES));
    cx.update_window(handle, |_, window, _| assert!(window.try_find("empty-state").is_some()))
        .unwrap();
}

#[gpui_kit::test]
fn brand_theme_is_gold_on_black(cx: &mut TestAppContext) {
    open_board(
        cx,
        vec![],
        StatusCache::default(),
        Arc::new(demo::DemoSource),
        options(),
    );
    let (primary, background) = cx.update(|cx| (cx.theme().primary, cx.theme().background));
    // #D4AF37 and #0A0A0A from themes/dashboard.json.
    assert_eq!(rgb_of(primary), (0xD4, 0xAF, 0x37));
    assert_eq!(rgb_of(background), (0x0A, 0x0A, 0x0A));
}

#[gpui_kit::test]
async fn cached_cards_render_before_any_refresh(cx: &mut TestAppContext) {
    let mut cache = StatusCache::default();
    cache.insert("/demo/gh-x".into(), fixtures::document(CLEAN));
    let (handle, view) = open_board(cx, demo::checkouts(), cache, Arc::new(demo::DemoSource), options());
    // No refresh has run: the cached card is complete, the rest are loading.
    assert_eq!(
        overall(cx, &view),
        [Overall::Clean, Overall::Loading, Overall::Loading, Overall::Loading]
    );
    cx.update_window(handle, |_, window, _| {
        assert!(window.try_find(("card", 0usize)).is_some());
        assert!(window.try_find(("card", 3usize)).is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
async fn refresh_fills_every_card_state_and_a_failure_stays_on_its_card(cx: &mut TestAppContext) {
    let (handle, view) = open_board(
        cx,
        demo::checkouts(),
        StatusCache::default(),
        Arc::new(demo::DemoSource),
        options(),
    );
    cx.update_window(handle, |_, window, cx| window.press("f5", cx))
        .unwrap();
    settle(cx, handle, &view).await;
    assert_eq!(
        overall(cx, &view),
        [Overall::Clean, Overall::Attention, Overall::Attention, Overall::Failed]
    );
    cx.update_window(handle, |_, window, _| {
        assert!(window.try_find(("card-error", 3usize)).is_some());
        assert!(window.try_find(("card-error", 0usize)).is_none());
        // One failing repository is not a setup problem.
        assert!(window.try_find("banner").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
async fn expanding_a_card_shows_its_tables_and_rows_open_in_the_browser(cx: &mut TestAppContext) {
    let (handle, view) = open_board(
        cx,
        demo::checkouts(),
        StatusCache::default(),
        Arc::new(demo::DemoSource),
        options(),
    );
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
    settle(cx, handle, &view).await;
    cx.update_window(handle, |_, window, cx| {
        assert!(window.try_find(("tables", 1usize)).is_none());
        window.click(("expand", 1usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(cx.update(|cx| view.read(cx).is_expanded(Path::new("/demo/codexbar"))));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(("tables", 1usize)).is_some());
        window.click("pr-1-232", cx);
    })
    .unwrap();
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://github.com/hemsoft-dev/codexbar/pull/232")
    );
    cx.update_window(handle, |_, window, cx| window.click("run-1-18312000002", cx))
        .unwrap();
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://github.com/hemsoft-dev/codexbar/actions/runs/18312000002")
    );
    cx.update_window(handle, |_, window, cx| window.click("issue-1-231", cx))
        .unwrap();
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://github.com/hemsoft-dev/codexbar/issues/231")
    );
}

/// Records each fetch's `--refresh` flag.
#[derive(Default)]
struct Recording(std::sync::Mutex<Vec<(PathBuf, bool)>>);

impl StatusSource for Recording {
    fn fetch(&self, checkout: &Path, refresh: bool) -> Result<StatusDocument, FetchError> {
        self.0.lock().unwrap().push((checkout.to_owned(), refresh));
        Ok(fixtures::document(CLEAN))
    }
}

#[gpui_kit::test]
async fn refreshing_one_card_bypasses_the_gh_x_cache_and_refresh_all_does_not(cx: &mut TestAppContext) {
    let source = Arc::new(Recording::default());
    let (handle, view) = open_board(cx, demo::checkouts(), StatusCache::default(), source.clone(), options());
    cx.update_window(handle, |_, window, cx| window.click(("refresh", 2usize), cx))
        .unwrap();
    settle(cx, handle, &view).await;
    assert_eq!(*source.0.lock().unwrap(), [(PathBuf::from("/demo/minibot"), true)]);
    cx.update_window(handle, |_, window, cx| window.click("refresh-all", cx))
        .unwrap();
    settle(cx, handle, &view).await;
    let calls = source.0.lock().unwrap();
    assert_eq!(calls.len(), 5);
    assert!(calls[1..].iter().all(|(_, refresh)| !refresh));
}

#[gpui_kit::test]
async fn a_missing_gh_shows_one_setup_message(cx: &mut TestAppContext) {
    struct NoGh;
    impl StatusSource for NoGh {
        fn fetch(&self, _: &Path, _: bool) -> Result<StatusDocument, FetchError> {
            Err(FetchError::GhMissing)
        }
    }
    let (handle, view) = open_board(cx, demo::checkouts(), StatusCache::default(), Arc::new(NoGh), options());
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
    settle(cx, handle, &view).await;
    cx.update_window(handle, |_, window, _| {
        assert_eq!(window.find_all("banner").len(), 1);
        // The cards don't repeat it.
        assert!(window.try_find(("card-error", 0usize)).is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
async fn automatic_refresh_runs_on_its_interval(cx: &mut TestAppContext) {
    let source = Arc::new(Recording::default());
    let mut options = options();
    options.auto_refresh = Some(Duration::from_secs(300));
    let (handle, view) = open_board(
        cx,
        vec!["/demo/gh-x".into()],
        StatusCache::default(),
        source.clone(),
        options,
    );
    cx.executor().advance_clock(Duration::from_secs(299));
    cx.run_until_parked();
    assert!(source.0.lock().unwrap().is_empty());
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    settle(cx, handle, &view).await;
    assert_eq!(*source.0.lock().unwrap(), [(PathBuf::from("/demo/gh-x"), false)]);
}

#[test]
fn counts_mark_unavailable_sections_unknown() {
    assert_eq!(
        counts_line(&fixtures::document(ATTENTION)),
        "2 PRs · 2 issues · 1 merged · 2 runs"
    );
    assert_eq!(
        counts_line(&fixtures::document(UNAVAILABLE)),
        "? PRs · ? issues · 0 merged · 0 runs"
    );
}

fn rgb_of(color: gpui_kit::Hsla) -> (u8, u8, u8) {
    let rgba = color.to_rgb();
    let channel = |value: f32| (value * 255.).round() as u8;
    (channel(rgba.r), channel(rgba.g), channel(rgba.b))
}
