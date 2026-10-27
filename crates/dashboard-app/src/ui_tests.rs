//! Headless UI tests: the real board view in an in-process test window, with the fixture-backed demo source. Nothing
//! here opens a desktop window, runs gh, sends OS input or changes focus outside the test.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use dashboard_core::cache::StatusCache;
use dashboard_core::columns::{Column, ColumnWidths};
use dashboard_core::fixtures::{self, ATTENTION, CLEAN, UNAVAILABLE};
use dashboard_core::source::{FetchError, StatusSource};
use dashboard_core::status::StatusDocument;
use dashboard_core::{EmptyState, Overall, SortOrder};
use gpui_kit::component::{ActiveTheme as _, Root};
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, px, size};

use crate::board_view::{self, BoardView, Options, section_count};
use crate::{demo, theme, zoom};

/// The demo's first four repositories: clean, attention, partly unavailable and failing.
pub(crate) fn four() -> Vec<PathBuf> {
    demo::checkouts().into_iter().take(4).collect()
}

pub(crate) fn options() -> Options {
    Options {
        cache_dir: None,
        parallelism: 4,
        auto_refresh: None,
        config_problem: None,
        config_path: None,
        config_dir: None,
        now: Some(demo::fixture_now()),
        columns: Default::default(),
        ui_state_dir: None,
        threads: Arc::new(demo::DemoThreads),
        priorities: Arc::new(demo::DemoPriorities),
        pmd: None,
        write: Arc::new(dashboard_core::write::Off),
        sample: None,
    }
}

pub(crate) fn open_board(
    cx: &mut TestAppContext,
    checkouts: Vec<PathBuf>,
    cache: StatusCache,
    source: Arc<dyn StatusSource>,
    options: Options,
) -> (AnyWindowHandle, Entity<BoardView>) {
    open_board_sized(cx, checkouts, cache, source, options, 1100.)
}

fn open_board_sized(
    cx: &mut TestAppContext,
    checkouts: Vec<PathBuf>,
    cache: StatusCache,
    source: Arc<dyn StatusSource>,
    options: Options,
    width: f32,
) -> (AnyWindowHandle, Entity<BoardView>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        theme::init(cx);
        board_view::bind_keys(cx);
        // In memory, unless a test set up its own folder first.
        crate::zoom::init_if_absent(cx, None);
    });
    let mut view = None;
    let handle = cx.open_window(size(px(width), px(900.)), |window, cx| {
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
pub(crate) async fn settle(cx: &mut TestAppContext, handle: AnyWindowHandle, view: &Entity<BoardView>) {
    for _ in 0..200 {
        std::thread::sleep(Duration::from_millis(2));
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        if !cx.update(|cx| view.read(cx).board().is_refreshing() || view.read(cx).priorities_pending()) {
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
    let (handle, view) = open_board(cx, four(), cache, Arc::new(demo::DemoSource), options());
    // No refresh has run: the cached card is complete, the rest are loading.
    assert_eq!(
        overall(cx, &view),
        [Overall::Clean, Overall::Loading, Overall::Loading, Overall::Loading]
    );
    cx.update_window(handle, |_, window, _| {
        assert!(window.try_find(("row", 0usize)).is_some());
        assert!(window.try_find(("row", 3usize)).is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
async fn refresh_fills_every_card_state_and_a_failure_stays_on_its_card(cx: &mut TestAppContext) {
    let (handle, view) = open_board(
        cx,
        four(),
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
        assert!(window.try_find(("row-error", 3usize)).is_some());
        assert!(window.try_find(("row-error", 0usize)).is_none());
        // One failing repository is not a setup problem.
        assert!(window.try_find("banner").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
async fn expanding_a_card_shows_its_tables_and_rows_open_in_the_browser(cx: &mut TestAppContext) {
    let (handle, view) = open_board(
        cx,
        four(),
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
async fn manual_and_full_refresh_preserve_the_source_request_intent(cx: &mut TestAppContext) {
    let source = Arc::new(Recording::default());
    let (handle, view) = open_board(cx, four(), StatusCache::default(), source.clone(), options());
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
async fn a_missing_pmd_token_shows_one_setup_message(cx: &mut TestAppContext) {
    struct NoToken;
    impl StatusSource for NoToken {
        fn fetch(&self, _: &Path, _: bool) -> Result<StatusDocument, FetchError> {
            Err(FetchError::MissingToken)
        }
    }
    let (handle, view) = open_board(cx, four(), StatusCache::default(), Arc::new(NoToken), options());
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
    settle(cx, handle, &view).await;
    cx.update_window(handle, |_, window, _| {
        assert_eq!(window.find_all("banner").len(), 1);
        // The rows say they have nothing rather than "Loading…" forever.
        for ix in 0..4usize {
            assert_eq!(window.find(("activity-placeholder", ix)).label(), Some("Unavailable"));
        }
        // The cards don't repeat it.
        assert!(window.try_find(("row-error", 0usize)).is_none());
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

#[gpui_kit::test]
async fn an_unavailable_section_reads_unavailable_in_the_expanded_card_not_none(cx: &mut TestAppContext) {
    let (handle, view) = open_board(
        cx,
        vec!["/demo/minibot".into()],
        StatusCache::default(),
        Arc::new(demo::DemoSource),
        options(),
    );
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
    settle(cx, handle, &view).await;
    cx.update_window(handle, |_, window, cx| window.click(("expand", 0usize), cx))
        .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("unavailable-Open PRs").is_some());
        assert!(window.try_find("unavailable-Open issues").is_some());
        // The merged list and runs loaded (empty), so they don't claim to be unavailable.
        assert!(window.try_find("unavailable-Recently merged").is_none());
        assert!(window.try_find("unavailable-CI runs").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
async fn cached_cards_keep_their_tag_while_a_refresh_runs(cx: &mut TestAppContext) {
    let mut cache = StatusCache::default();
    cache.insert("/demo/codexbar".into(), fixtures::document(ATTENTION));
    let (handle, view) = open_board(
        cx,
        vec!["/demo/codexbar".into()],
        cache,
        Arc::new(demo::DemoSource),
        options(),
    );
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(("cached", 0usize)).is_some(), "tagged while refreshing");
    })
    .unwrap();
    assert!(cx.update(|cx| view.read(cx).board().is_refreshing()));
    settle(cx, handle, &view).await;
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find(("cached", 0usize)).is_none(),
            "fresh data drops the tag"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
async fn the_updated_label_keeps_ticking_after_a_refresh(cx: &mut TestAppContext) {
    let (handle, view) = open_board(
        cx,
        vec!["/demo/gh-x".into()],
        StatusCache::default(),
        Arc::new(demo::DemoSource),
        options(),
    );
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
    settle(cx, handle, &view).await;
    let notified = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = notified.clone();
    let _observer = cx.update(|cx| {
        cx.observe(&view, move |_, _| {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        })
    });
    cx.executor().advance_clock(Duration::from_secs(61));
    cx.run_until_parked();
    assert!(
        notified.load(std::sync::atomic::Ordering::SeqCst) >= 2,
        "re-rendered as time passed"
    );
}

/// Serves the real gh-x v0.22.0 `--merged 0` output for every checkout.
struct MergedOff;

impl StatusSource for MergedOff {
    fn fetch(&self, _: &Path, _: bool) -> Result<StatusDocument, FetchError> {
        Ok(StatusDocument::parse(include_str!(
            "../../dashboard-core/tests/fixtures/real-gh-x-v0.22.0-merged0.json"
        ))
        .unwrap())
    }
}

#[gpui_kit::test]
async fn a_turned_off_section_reads_off_in_the_expanded_card(cx: &mut TestAppContext) {
    let (handle, view) = open_board(
        cx,
        vec!["/demo/project-dashboard".into()],
        StatusCache::default(),
        Arc::new(MergedOff),
        options(),
    );
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
    settle(cx, handle, &view).await;
    cx.update_window(handle, |_, window, cx| window.click(("expand", 0usize), cx))
        .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("disabled-Recently merged").is_some());
        assert!(window.try_find("unavailable-Recently merged").is_none());
        assert!(window.try_find("disabled-Open issues").is_none());
    })
    .unwrap();
}

#[test]
fn counts_mark_unavailable_and_turned_off_sections() {
    let attention = fixtures::document(ATTENTION);
    assert_eq!(section_count(&attention, "pullRequests.open", 2), "2");
    let unavailable = fixtures::document(UNAVAILABLE);
    assert_eq!(section_count(&unavailable, "pullRequests.open", 0), "?");
    assert_eq!(section_count(&unavailable, "issues", 0), "?");
    let merged_off = StatusDocument::parse(include_str!(
        "../../dashboard-core/tests/fixtures/real-gh-x-v0.22.0-merged0.json"
    ))
    .unwrap();
    assert_eq!(section_count(&merged_off, "pullRequests.merged", 0), "–");
}

/// Every demo row filled, as F5 would.
async fn twelve(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<BoardView>) {
    let (handle, view) = open_board(
        cx,
        demo::checkouts(),
        StatusCache::default(),
        Arc::new(demo::DemoSource),
        options(),
    );
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
    settle(cx, handle, &view).await;
    (handle, view)
}

fn text_of(cx: &mut TestAppContext, handle: AnyWindowHandle, id: (&'static str, usize)) -> String {
    cx.update_window(handle, |_, window, _| {
        window.find(id).label().unwrap_or_default().to_owned()
    })
    .unwrap()
}

#[gpui_kit::test]
async fn the_table_has_one_row_per_repository_with_counts_update_time_and_last_activity(cx: &mut TestAppContext) {
    let (handle, view) = twelve(cx).await;
    assert_eq!(cx.update(|cx| view.read(cx).board().cards().len()), 12);
    cx.update_window(handle, |_, window, _| {
        for ix in 0..12usize {
            assert!(window.try_find(("row", ix)).is_some(), "row {ix}");
        }
        // Collapsed: no details anywhere.
        assert!(window.try_find(("details", 0usize)).is_none());
    })
    .unwrap();
    // codexbar (attention fixture): 2 PRs, 2 issues, and its newest item is the dependabot run at 06:26:02.
    assert_eq!(text_of(cx, handle, ("prs", 1)), "2 open pull requests");
    assert_eq!(text_of(cx, handle, ("issues", 1)), "2 open issues");
    // minibot: both sections unavailable.
    assert_eq!(text_of(cx, handle, ("prs", 2)), "? open pull requests");
    // The age has its own column; the last activity keeps the item and its title.
    assert_eq!(
        text_of(cx, handle, ("activity", 7)),
        "Last activity: Issue #231, Show the tray icon on Linux"
    );
    assert_eq!(text_of(cx, handle, ("updated", 7)), "Updated 14m ago");
    assert_eq!(text_of(cx, handle, ("updated", 1)), "Updated 3m ago");
    assert_eq!(text_of(cx, handle, ("updated", 4)), "Updated 3m ago");
    assert_eq!(text_of(cx, handle, ("updated", 0)), "Updated 2d ago");
    // Nothing to date: minibot's sections are unavailable and weather-cli failed.
    assert_eq!(text_of(cx, handle, ("updated", 2)), "Updated: unknown");
    assert_eq!(text_of(cx, handle, ("updated", 3)), "Updated: unknown");
    let labels = cx.update(|cx| {
        view.read(cx)
            .board()
            .cards()
            .iter()
            .map(|card| card.last_activity().map(|activity| activity.label))
            .collect::<Vec<_>>()
    });
    assert_eq!(labels[1].as_deref(), Some("CI run"));
    assert_eq!(labels[4].as_deref(), Some("PR #232"));
    assert_eq!(labels[2], None);
    // Every row's update time starts at the same x, under its header.
    cx.update_window(handle, |_, window, _| {
        let header = window.find("sort-updated").bounds().origin.x;
        for ix in [0usize, 1, 4, 7, 11] {
            assert_eq!(window.find(("updated", ix)).bounds().origin.x, header, "row {ix}");
        }
    })
    .unwrap();
}

#[gpui_kit::test]
async fn rows_expand_and_collapse_and_several_stay_open_across_refreshes(cx: &mut TestAppContext) {
    let (handle, view) = twelve(cx).await;
    cx.update_window(handle, |_, window, cx| {
        window.click(("row", 1usize), cx);
        window.click(("expand", 3usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(("details", 1usize)).is_some());
        assert!(window.try_find(("details", 3usize)).is_some());
        assert!(window.try_find(("details", 0usize)).is_none());
        // The failing row's details show its error; codexbar's show the tables.
        assert!(window.try_find(("tables", 1usize)).is_some());
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| window.press("f5", cx))
        .unwrap();
    settle(cx, handle, &view).await;
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find(("details", 1usize)).is_some(),
            "still open after a refresh"
        );
        assert!(window.try_find(("details", 3usize)).is_some());
        window.click(("row", 1usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find(("details", 1usize)).is_none(),
            "a second click collapses"
        );
        assert!(window.try_find(("details", 3usize)).is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
async fn the_updated_header_sorts_newest_first_then_oldest_first_then_config_order(cx: &mut TestAppContext) {
    let (handle, view) = twelve(cx).await;
    let names = |cx: &mut TestAppContext| {
        cx.update(|cx| view.read(cx).row_order())
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
    };
    let header = |cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.find("sort-updated").label().unwrap_or_default().to_owned()
        })
        .unwrap()
    };
    let click = |cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| window.click("sort-updated", cx))
            .unwrap();
        cx.run_until_parked();
    };
    assert_eq!(cx.update(|cx| view.read(cx).sort()), SortOrder::Config);
    assert_eq!(header(cx), "Updated, not sorted");
    assert_eq!(names(cx)[..4], ["gh-x", "codexbar", "minibot", "weather-cli"]);
    click(cx);
    assert_eq!(cx.update(|cx| view.read(cx).sort()), SortOrder::NewestFirst);
    assert_eq!(header(cx), "Updated, newest first");
    assert_eq!(
        names(cx),
        [
            // hs-buddy's PR at 06:27:00 is just newer than codexbar's run at 06:26:02 (both "3m ago").
            "hs-buddy",
            "codexbar",
            "project-dashboard",
            "chief",
            "hs-pi-dashboard",
            "set-it-free-loop",
            "summarist",
            // Oct 19 06:10 (tickdown) before Oct 19 04:12 (gh-x), then Oct 15 (yahtzee).
            "tickdown",
            "gh-x",
            "yahtzee",
            // No activity: minibot (sections unavailable) and weather-cli (failed).
            "minibot",
            "weather-cli",
        ]
    );
    click(cx);
    assert_eq!(cx.update(|cx| view.read(cx).sort()), SortOrder::OldestFirst);
    assert_eq!(header(cx), "Updated, oldest first");
    assert_eq!(
        names(cx),
        [
            "yahtzee",
            "gh-x",
            "tickdown",
            "summarist",
            "set-it-free-loop",
            "hs-pi-dashboard",
            "chief",
            "project-dashboard",
            "codexbar",
            "hs-buddy",
            // Rows without an update time stay last in this direction too.
            "minibot",
            "weather-cli",
        ]
    );
    click(cx);
    assert_eq!(cx.update(|cx| view.read(cx).sort()), SortOrder::Config);
    assert_eq!(header(cx), "Updated, not sorted");
    assert_eq!(
        names(cx),
        demo::checkouts()
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
    );
}

#[gpui_kit::test]
async fn clicking_the_last_activity_opens_it_without_expanding_the_row(cx: &mut TestAppContext) {
    let (handle, view) = twelve(cx).await;
    cx.update_window(handle, |_, window, cx| window.click(("activity", 7usize), cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://github.com/hemsoft-dev/codexbar/issues/231")
    );
    assert!(!cx.update(|cx| view.read(cx).is_expanded(Path::new("/demo/project-dashboard"))));
}

#[gpui_kit::test]
async fn at_the_minimum_width_every_row_can_scroll_its_refresh_button_into_view(cx: &mut TestAppContext) {
    // 480 px is the window's minimum (main.rs).
    let (handle, view) = open_board_sized(
        cx,
        demo::checkouts(),
        StatusCache::default(),
        Arc::new(demo::DemoSource),
        options(),
        480.,
    );
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
    settle(cx, handle, &view).await;
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.scroll(
            "table",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(-2000.), px(0.))),
            cx,
        );
        window.render_frame(cx);
        for ix in 0..12usize {
            let bounds = window.find(("refresh", ix)).bounds();
            assert!(
                bounds.origin.x + bounds.size.width <= px(480.),
                "row {ix}: refresh button ends at {:?}",
                bounds.origin.x + bounds.size.width
            );
        }
        // Every last-activity label ends before its row's refresh button.
        for ix in [0usize, 1, 4, 5, 6, 7, 8, 9, 10, 11] {
            let summary = window.find(("activity-summary", ix)).bounds();
            let refresh = window.find(("refresh", ix)).bounds();
            assert!(
                summary.origin.x + summary.size.width <= refresh.origin.x,
                "row {ix}: summary ends at {:?}, refresh starts at {:?}",
                summary.origin.x + summary.size.width,
                refresh.origin.x
            );
        }
        // The name keeps room to read, rather than collapsing to nothing.
        for ix in 0..12usize {
            let name = window.find(("name", ix)).bounds();
            // NAME_MIN is 6 rem: 96 px at the 16 px base.
            assert!(
                name.size.width >= px(96.),
                "row {ix}: name is {:?} wide",
                name.size.width
            );
        }
        // The state is named for screen readers, not only colored.
        assert_eq!(window.find("state-3-Failed").label(), Some("State: Refresh failed"));
    })
    .unwrap();
}

#[gpui_kit::test]
async fn a_failing_row_shows_its_error_and_keeps_its_last_counts(cx: &mut TestAppContext) {
    let mut cache = StatusCache::default();
    cache.insert("/demo/weather-cli".into(), fixtures::document(ATTENTION));
    let (handle, view) = open_board(
        cx,
        vec!["/demo/weather-cli".into()],
        cache,
        Arc::new(demo::DemoSource),
        options(),
    );
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
    settle(cx, handle, &view).await;
    assert!(text_of(cx, handle, ("row-error", 0)).starts_with("HTTP 401: Bad credentials"));
    assert_eq!(text_of(cx, handle, ("prs", 0)), "2 open pull requests");
    cx.update_window(handle, |_, window, _| {
        assert!(window.try_find("state-0-Failed").is_some());
    })
    .unwrap();
}

fn rgb_of(color: gpui_kit::Hsla) -> (u8, u8, u8) {
    let rgba = color.to_rgb();
    let channel = |value: f32| (value * 255.).round() as u8;
    (channel(rgba.r), channel(rgba.g), channel(rgba.b))
}

fn press(cx: &mut TestAppContext, handle: AnyWindowHandle, key: &str) {
    cx.update_window(handle, |_, window, cx| window.press(key, cx)).unwrap();
    cx.run_until_parked();
}

fn font_size(cx: &mut TestAppContext) -> f32 {
    cx.update(|cx| f32::from(cx.theme().font_size))
}

#[gpui_kit::test]
async fn ctrl_plus_minus_and_zero_step_the_whole_ui_within_50_to_200_percent(cx: &mut TestAppContext) {
    let (handle, _) = twelve(cx).await;
    assert_eq!(cx.update(|cx| zoom::level(cx)), 1.0);
    assert_eq!(font_size(cx), 16.0);
    for key in ["ctrl-=", "ctrl-+", "ctrl-shift-="] {
        press(cx, handle, key);
    }
    assert_eq!(cx.update(|cx| zoom::level(cx)), 1.3);
    assert_eq!(font_size(cx), 16.0 * 1.3);
    press(cx, handle, "ctrl--");
    assert_eq!(cx.update(|cx| zoom::level(cx)), 1.2);
    press(cx, handle, "ctrl-0");
    assert_eq!(cx.update(|cx| zoom::level(cx)), 1.0);
    for _ in 0..20 {
        press(cx, handle, "ctrl-=");
    }
    assert_eq!(cx.update(|cx| zoom::level(cx)), 2.0);
    for _ in 0..30 {
        press(cx, handle, "ctrl--");
    }
    assert_eq!(cx.update(|cx| zoom::level(cx)), 0.5);
    assert_eq!(font_size(cx), 8.0);
}

#[gpui_kit::test]
async fn the_level_shows_briefly_as_a_toast(cx: &mut TestAppContext) {
    let (handle, _) = twelve(cx).await;
    press(cx, handle, "ctrl-=");
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("zoom-toast").label(), Some("110%"));
    })
    .unwrap();
    cx.executor().advance_clock(zoom::TOAST + Duration::from_millis(50));
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("zoom-toast").is_none(), "gone after {:?}", zoom::TOAST);
    })
    .unwrap();
}

#[gpui_kit::test]
async fn ctrl_wheel_zooms_and_a_plain_wheel_does_not(cx: &mut TestAppContext) {
    use gpui_kit::{InputEvent as _, Modifiers, ScrollDelta, ScrollWheelEvent, TouchPhase, point};
    let (handle, _) = twelve(cx).await;
    let wheel = |cx: &mut TestAppContext, control: bool, notches: f32| {
        cx.update_window(handle, |_, window, cx| {
            let event = ScrollWheelEvent {
                position: point(px(500.), px(300.)),
                delta: ScrollDelta::Lines(point(0., notches)),
                modifiers: Modifiers {
                    control,
                    ..Modifiers::default()
                },
                touch_phase: TouchPhase::Moved,
            };
            window.dispatch_event(event.to_platform_input(), cx);
        })
        .unwrap();
        cx.run_until_parked();
    };
    wheel(cx, true, 1.);
    wheel(cx, true, 1.);
    assert_eq!(cx.update(|cx| zoom::level(cx)), 1.2);
    wheel(cx, true, -1.);
    assert_eq!(cx.update(|cx| zoom::level(cx)), 1.1);
    wheel(cx, false, 3.);
    assert_eq!(
        cx.update(|cx| zoom::level(cx)),
        1.1,
        "scrolling without Ctrl leaves the zoom alone"
    );
}

#[gpui_kit::test]
async fn the_zoom_is_saved_and_restored_on_the_next_start(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("project-dashboard-ui-zoom-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    cx.update(|cx| {
        gpui_kit::init(cx);
        theme::init(cx);
        zoom::init_if_absent(cx, Some(dir.clone()));
    });
    let (handle, _) = twelve(cx).await;
    for _ in 0..3 {
        press(cx, handle, "ctrl-=");
    }
    // Saved once input settles, not on every step.
    assert!(!dir.join(dashboard_core::ui_state::STATE_FILE).exists());
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(dashboard_core::ui_state::UiState::load(&dir).zoom, 1.3);
    // The next start reads it back.
    cx.update(|cx| {
        zoom::reset_for_tests(cx);
        zoom::init_if_absent(cx, Some(dir.clone()));
    });
    assert_eq!(cx.update(|cx| zoom::level(cx)), 1.3);
    assert_eq!(font_size(cx), 16.0 * 1.3);
    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui_kit::test]
async fn at_200_percent_the_table_keeps_readable_columns_and_scrolls_to_controls(cx: &mut TestAppContext) {
    let (handle, _) = twelve(cx).await;
    cx.update(|cx| zoom::set_level(2.0, cx));
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.scroll(
            "table",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(-2000.), px(0.))),
            cx,
        );
        window.render_frame(cx);
        for ix in 0..12usize {
            let refresh = window.find(("refresh", ix)).bounds();
            assert!(refresh.origin.x + refresh.size.width <= px(1100.), "row {ix} at 200%");
            // Rows grew with the text: a 200% row is taller than the 100% one (about 37 px).
            assert!(
                window.find(("row", ix)).bounds().size.height > px(60.),
                "row {ix} height"
            );
            // Names keep a readable share of the width rather than their minimum alone.
            assert!(
                window.find(("name", ix)).bounds().size.width >= px(6. * 32.),
                "row {ix} name width"
            );
        }
    })
    .unwrap();
}

#[gpui_kit::test]
async fn at_200_percent_in_the_narrowest_window_the_table_scrolls_sideways_to_every_control(cx: &mut TestAppContext) {
    use gpui_kit::{ScrollDelta, point};
    let (handle, view) = open_board_sized(
        cx,
        demo::checkouts(),
        StatusCache::default(),
        Arc::new(demo::DemoSource),
        options(),
        480.,
    );
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
    settle(cx, handle, &view).await;
    cx.update(|cx| zoom::set_level(2.0, cx));
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // Wider than the window: the refresh buttons start out of view, and names keep their minimum.
        let refresh = window.find(("refresh", 0usize)).bounds();
        assert!(
            refresh.origin.x > px(480.),
            "starts off to the right at {:?}",
            refresh.origin.x
        );
        assert!(window.find(("name", 0usize)).bounds().size.width >= px(6. * 32.));
        window.scroll("table", ScrollDelta::Pixels(point(px(-2000.), px(0.))), cx);
        window.render_frame(cx);
        let refresh = window.find(("refresh", 0usize)).bounds();
        assert!(
            refresh.origin.x >= px(0.) && refresh.origin.x + refresh.size.width <= px(480.),
            "scrolled into view: {:?}",
            refresh
        );
    })
    .unwrap();
}

#[gpui_kit::test]
async fn at_200_percent_in_the_narrowest_window_the_title_bar_keeps_its_refresh_button(cx: &mut TestAppContext) {
    let (handle, view) = open_board_sized(
        cx,
        demo::checkouts(),
        StatusCache::default(),
        Arc::new(demo::DemoSource),
        options(),
        480.,
    );
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
    settle(cx, handle, &view).await;
    cx.update(|cx| zoom::set_level(2.0, cx));
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let button = window.find("refresh-all").bounds();
        assert!(
            button.origin.x + button.size.width <= px(480.),
            "refresh-all ends at {:?}",
            button.origin.x + button.size.width
        );
        let title = window.find("app-title").bounds();
        let status = window.find("refresh-status").bounds();
        assert!(
            title.origin.x + title.size.width <= status.origin.x + px(1.),
            "title overlaps the status"
        );
        assert!(
            status.origin.x + status.size.width <= button.origin.x + px(1.),
            "status overlaps the button"
        );
    })
    .unwrap();
}

// Resizable columns (#14).

/// The center of a column's resize handle, in window coordinates.
fn handle_center(
    cx: &mut TestAppContext,
    handle: AnyWindowHandle,
    column: Column,
) -> gpui_kit::Point<gpui_kit::Pixels> {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.find(("resize", column as usize)).bounds().center()
    })
    .unwrap()
}

fn drag_border(cx: &mut TestAppContext, handle: AnyWindowHandle, column: Column, dx: f32) {
    let from = handle_center(cx, handle, column);
    cx.update_window(handle, |_, window, cx| {
        window.drag(from, gpui_kit::point(from.x + px(dx), from.y), cx)
    })
    .unwrap();
    cx.run_until_parked();
}

fn width_of(cx: &mut TestAppContext, handle: AnyWindowHandle, id: impl Into<gpui_kit::ElementId>) -> gpui_kit::Pixels {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.find(id).bounds().size.width
    })
    .unwrap()
}

#[gpui_kit::test]
async fn the_repository_column_defaults_to_a_name_sized_width(cx: &mut TestAppContext) {
    let (handle, view) = twelve(cx).await;
    assert_eq!(cx.update(|cx| view.read(cx).columns()), ColumnWidths::default());
    // 18 rem at the 16 px base: no wider than about 20 rem in a 1100 px window, leaving the rest to last activity.
    assert_eq!(width_of(cx, handle, "header-repository"), px(18. * 16.));
    for ix in 0..12usize {
        assert_eq!(width_of(cx, handle, ("name", ix)), px(18. * 16.), "row {ix}");
    }
    cx.update_window(handle, |_, window, _| {
        // Every handle shows the resize cursor's purpose to assistive tech too.
        assert_eq!(
            window.find(("resize", Column::Repository as usize)).label(),
            Some("Resize Repository")
        );
        // Last activity got the width the narrower repository column freed.
        let summary = window.find(("activity-summary", 0usize)).bounds();
        assert!(
            summary.origin.x < px(700.),
            "last activity starts at {:?}",
            summary.origin.x
        );
    })
    .unwrap();
}

#[gpui_kit::test]
async fn dragging_a_column_border_resizes_the_header_and_every_row(cx: &mut TestAppContext) {
    let (handle, view) = twelve(cx).await;
    drag_border(cx, handle, Column::Repository, -80.);
    let columns = cx.update(|cx| view.read(cx).columns());
    assert_eq!(columns.get(Column::Repository), 13.0);
    assert_eq!(width_of(cx, handle, "header-repository"), px(13. * 16.));
    for ix in 0..12usize {
        assert_eq!(width_of(cx, handle, ("name", ix)), px(13. * 16.), "row {ix}");
    }
    // Another column, wider: its header and cells grow together, and the handle followed the border.
    let before = handle_center(cx, handle, Column::PullRequests);
    drag_border(cx, handle, Column::PullRequests, 32.);
    assert_eq!(cx.update(|cx| view.read(cx).columns()).get(Column::PullRequests), 5.25);
    assert_eq!(width_of(cx, handle, ("prs", 3usize)), px(5.25 * 16.));
    assert_eq!(handle_center(cx, handle, Column::PullRequests).x, before.x + px(32.));
    // Dragging a border never sorts or expands anything.
    assert_eq!(cx.update(|cx| view.read(cx).sort()), SortOrder::Config);
    assert!(!cx.update(|cx| view.read(cx).is_expanded(Path::new("/demo/gh-x"))));
}

#[gpui_kit::test]
async fn a_column_stops_at_its_minimum_and_a_double_click_restores_its_default(cx: &mut TestAppContext) {
    let (handle, view) = twelve(cx).await;
    drag_border(cx, handle, Column::Repository, -1000.);
    assert_eq!(
        cx.update(|cx| view.read(cx).columns()).get(Column::Repository),
        Column::Repository.min_width()
    );
    assert_eq!(width_of(cx, handle, ("name", 0usize)), px(6. * 16.));
    drag_border(cx, handle, Column::Updated, -1000.);
    assert_eq!(
        width_of(cx, handle, "sort-updated"),
        px(Column::Updated.min_width() * 16.)
    );
    cx.update_window(handle, |_, window, cx| {
        window.double_click(("resize", Column::Repository as usize), cx)
    })
    .unwrap();
    cx.run_until_parked();
    let columns = cx.update(|cx| view.read(cx).columns());
    assert_eq!(columns.get(Column::Repository), Column::Repository.default_width());
    assert_eq!(
        columns.get(Column::Updated),
        Column::Updated.min_width(),
        "only that column"
    );
    assert_eq!(width_of(cx, handle, ("name", 0usize)), px(18. * 16.));
}

#[gpui_kit::test]
async fn on_a_narrow_window_a_repository_drag_starts_from_the_width_on_screen(cx: &mut TestAppContext) {
    // 640 px leaves the repository column less than its 18 rem (288 px) setting.
    let (handle, view) = open_board_sized(
        cx,
        demo::checkouts(),
        StatusCache::default(),
        Arc::new(demo::DemoSource),
        options(),
        640.,
    );
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
    settle(cx, handle, &view).await;
    let shown = width_of(cx, handle, "header-repository");
    assert!(shown < px(18. * 16.), "shrunk to {shown:?}");
    drag_border(cx, handle, Column::Repository, -16.);
    // One rem left of what was on screen, not one rem left of the setting (which would show no change at all).
    let after = width_of(cx, handle, "header-repository");
    assert_eq!(after, shown - px(16.));
    assert_eq!(
        cx.update(|cx| view.read(cx).columns()).get(Column::Repository),
        Column::Repository.clamp(f32::from(shown) / 16. - 1.)
    );
}

#[gpui_kit::test]
async fn resized_columns_are_saved_and_restored_on_the_next_start(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("project-dashboard-ui-columns-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // The zoom saved earlier must survive a column save.
    dashboard_core::ui_state::UiState::update(&dir, |state| state.zoom = 1.2).unwrap();
    let saving = Options {
        ui_state_dir: Some(dir.clone()),
        ..options()
    };
    let (handle, view) = open_board(
        cx,
        demo::checkouts(),
        StatusCache::default(),
        Arc::new(demo::DemoSource),
        saving,
    );
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
    settle(cx, handle, &view).await;
    drag_border(cx, handle, Column::Repository, -96.);
    let saved = dashboard_core::ui_state::UiState::load(&dir);
    assert_eq!(saved.columns.get(Column::Repository), 12.0);
    assert_eq!(saved.zoom, 1.2);
    // The next start opens with the saved widths.
    let restored = Options {
        columns: saved.columns,
        ..options()
    };
    let (handle, view) = open_board(
        cx,
        demo::checkouts(),
        StatusCache::default(),
        Arc::new(demo::DemoSource),
        restored,
    );
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
    settle(cx, handle, &view).await;
    assert_eq!(width_of(cx, handle, ("name", 0usize)), px(12. * 16.));
    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui_kit::test]
async fn columns_wider_than_the_window_scroll_sideways_with_the_header(cx: &mut TestAppContext) {
    use gpui_kit::{ScrollDelta, point};
    let mut wide = ColumnWidths::default();
    // With the repository column at its minimum the table needs about 1,250 px.
    wide.set(Column::Updated, 55.);
    let (handle, view) = open_board(
        cx,
        demo::checkouts(),
        StatusCache::default(),
        Arc::new(demo::DemoSource),
        Options {
            columns: wide,
            ..options()
        },
    );
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
    settle(cx, handle, &view).await;
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let refresh = window.find(("refresh", 0usize)).bounds();
        assert!(
            refresh.origin.x > px(1100.),
            "starts off to the right at {:?}",
            refresh.origin.x
        );
        window.scroll("table", ScrollDelta::Pixels(point(px(-2000.), px(0.))), cx);
        window.render_frame(cx);
        let refresh = window.find(("refresh", 0usize)).bounds();
        assert!(
            refresh.origin.x + refresh.size.width <= px(1100.),
            "scrolled into view: {refresh:?}"
        );
        // The header scrolled with the rows: its Updated cell still lines up with theirs.
        assert_eq!(
            window.find("sort-updated").bounds().origin.x,
            window.find(("updated", 0usize)).bounds().origin.x
        );
    })
    .unwrap();
}

// The T3 Code thread button (#15).

/// Counts lookups, and answers like the demo.
struct CountingThreads(std::sync::atomic::AtomicUsize);

impl dashboard_core::t3::ThreadFinder for CountingThreads {
    fn find(&self, checkouts: &[PathBuf]) -> Vec<dashboard_core::t3::Lookup> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        demo::DemoThreads.find(checkouts)
    }
}

#[gpui_kit::test]
async fn each_row_has_a_t3_button_just_left_of_its_refresh_button(cx: &mut TestAppContext) {
    for width in [1100., 480.] {
        let (handle, view) = open_board_sized(
            cx,
            demo::checkouts(),
            StatusCache::default(),
            Arc::new(demo::DemoSource),
            options(),
            width,
        );
        cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
        settle(cx, handle, &view).await;
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.scroll(
                "table",
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(-2000.), px(0.))),
                cx,
            );
            window.render_frame(cx);
            for ix in 0..12usize {
                let t3 = window.find(("t3", ix)).bounds();
                let refresh = window.find(("refresh", ix)).bounds();
                // Same size, same line, directly before it, and in view.
                assert_eq!(t3.size, refresh.size, "row {ix} at {width}");
                assert_eq!(t3.center().y, refresh.center().y, "row {ix} at {width}");
                assert!(t3.origin.x + t3.size.width <= refresh.origin.x, "row {ix} at {width}");
                assert!(
                    refresh.origin.x - (t3.origin.x + t3.size.width) < px(8.),
                    "row {ix} at {width}"
                );
                assert!(
                    t3.origin.x >= px(0.) && refresh.origin.x + refresh.size.width <= px(width),
                    "row {ix}"
                );
            }
        })
        .unwrap();
    }
}

#[gpui_kit::test]
async fn the_t3_button_opens_the_checkouts_thread_without_expanding_the_row(cx: &mut TestAppContext) {
    let (handle, view) = twelve(cx).await;
    let gh_x = Path::new("/demo/gh-x");
    let link = cx.update(|cx| view.read(cx).thread(gh_x).cloned()).unwrap().unwrap();
    assert!(link.running);
    cx.update_window(handle, |_, window, cx| window.click(("t3", 0usize), cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(cx.opened_url().as_deref(), Some(link.url.as_str()));
    assert_eq!(link.url, "http://127.0.0.1:3773/demo-environment/demo%3Agh-x");
    assert!(!cx.update(|cx| view.read(cx).is_expanded(gh_x)));
}

#[gpui_kit::test]
async fn a_row_without_a_thread_has_a_disabled_t3_button_that_says_why(cx: &mut TestAppContext) {
    use dashboard_core::t3::{NoThread, tooltip};
    let (handle, view) = twelve(cx).await;
    let weather = Path::new("/demo/weather-cli");
    let minibot = Path::new("/demo/minibot");
    let lookup = |cx: &mut TestAppContext, path: &Path| cx.update(|cx| view.read(cx).thread(path).cloned()).unwrap();
    assert_eq!(lookup(cx, weather), Err(NoThread::NoProject));
    assert_eq!(lookup(cx, minibot), Err(NoThread::NoOpenThread));
    assert_eq!(tooltip(&lookup(cx, weather)), "No T3 Code project for this checkout");
    assert_eq!(
        tooltip(&lookup(cx, Path::new("/demo/codexbar"))),
        "Open the T3 Code thread: codexbar: L1 audit -> issues"
    );
    // Clicking the disabled button opens nothing and leaves the row as it was.
    cx.update_window(handle, |_, window, cx| window.click(("t3", 3usize), cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(cx.opened_url(), None);
    assert!(!cx.update(|cx| view.read(cx).is_expanded(weather)));
}

#[gpui_kit::test]
async fn threads_are_looked_up_at_start_and_again_after_each_refresh(cx: &mut TestAppContext) {
    let counter = Arc::new(CountingThreads(std::sync::atomic::AtomicUsize::new(0)));
    let (handle, view) = open_board(
        cx,
        demo::checkouts(),
        StatusCache::default(),
        Arc::new(demo::DemoSource),
        Options {
            threads: counter.clone(),
            ..options()
        },
    );
    cx.run_until_parked();
    let count = |counter: &CountingThreads| counter.0.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(count(&counter), 1, "at start");
    assert!(cx.update(|cx| view.read(cx).thread(Path::new("/demo/gh-x")).is_some()));
    cx.update_window(handle, |_, window, cx| window.press("f5", cx))
        .unwrap();
    settle(cx, handle, &view).await;
    // The refresh's last poll tick, which runs `finished` and starts the lookup, can come after the cards stop
    // refreshing; keep the clock moving until it has, as `settle` does for the refresh itself.
    for _ in 0..200 {
        if count(&counter) == 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
    }
    assert_eq!(count(&counter), 2, "after a refresh");
    // Asked twice while the first lookup is still running: the second waits for it, then runs, rather than being
    // dropped and leaving the first lookup's older answer in place.
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.find_threads(cx);
            view.find_threads(cx);
        })
    });
    cx.run_until_parked();
    assert_eq!(count(&counter), 4, "the queued lookup ran too");
    // Once idle, nothing more runs on its own.
    cx.run_until_parked();
    assert_eq!(count(&counter), 4);
}

// Reloading config.toml while the app runs (#21).

/// A config folder for one test, removed when it ends.
struct ConfigDir(PathBuf);

impl ConfigDir {
    fn new(name: &str, repositories: &[&str]) -> Self {
        let dir = std::env::temp_dir().join(format!("project-dashboard-ui-config-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config = Self(dir);
        config.write(repositories);
        config
    }

    fn write(&self, repositories: &[&str]) {
        let list: Vec<String> = repositories.iter().map(|repo| format!("\"{repo}\"")).collect();
        self.write_text(&format!("refresh_minutes = 0\nrepositories = [{}]\n", list.join(", ")));
    }

    fn write_text(&self, text: &str) {
        // A different length from the last write, so the edit is noticed even within one timestamp tick.
        let padding = " ".repeat(std::fs::metadata(self.0.join("config.toml")).map_or(0, |m| m.len() as usize % 7 + 1));
        std::fs::write(self.0.join("config.toml"), format!("{text}#{padding}\n")).unwrap();
    }
}

impl Drop for ConfigDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn open_watching(
    cx: &mut TestAppContext,
    config: &ConfigDir,
    startup: &[&str],
) -> (AnyWindowHandle, Entity<BoardView>) {
    open_board(
        cx,
        startup.iter().map(PathBuf::from).collect(),
        StatusCache::default(),
        Arc::new(demo::DemoSource),
        Options {
            config_dir: Some(config.0.clone()),
            ..options()
        },
    )
}

/// Lets two config polls (a change applies once two polls read it the same) and any refresh they start run to the
/// end.
async fn after_poll(cx: &mut TestAppContext, handle: AnyWindowHandle, view: &Entity<BoardView>) {
    for _ in 0..3 {
        cx.executor().advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
    }
    settle(cx, handle, view).await;
}

/// Keeps the test clock and the real fetch threads going until `done` holds, for up to about ten seconds of real
/// time: under a loaded test run the fetch threads can outlast `settle`'s budget.
fn until(cx: &mut TestAppContext, mut done: impl FnMut(&mut TestAppContext) -> bool) {
    for _ in 0..2000 {
        if done(cx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
    }
    assert!(done(cx), "timed out waiting");
}

fn listed(cx: &mut TestAppContext, view: &Entity<BoardView>) -> Vec<String> {
    cx.update(|cx| view.read(cx).board().checkouts())
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

fn banner(cx: &mut TestAppContext, handle: AnyWindowHandle) -> bool {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.try_find("banner").is_some()
    })
    .unwrap()
}

#[gpui_kit::test]
async fn a_repository_added_to_the_config_appears_without_a_restart(cx: &mut TestAppContext) {
    let config = ConfigDir::new("add", &["/demo/gh-x", "/demo/codexbar"]);
    let (handle, view) = open_watching(cx, &config, &["/demo/gh-x", "/demo/codexbar"]);
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
    settle(cx, handle, &view).await;
    cx.update(|cx| view.update(cx, |view, cx| view.toggle_expanded(Path::new("/demo/gh-x"), cx)));
    // Edit the file, as Franz did: no refresh, no restart.
    config.write(&["/demo/gh-x", "/demo/minibot", "/demo/codexbar"]);
    after_poll(cx, handle, &view).await;
    assert_eq!(listed(cx, &view), ["gh-x", "minibot", "codexbar"]);
    cx.update(|cx| {
        let view = view.read(cx);
        let minibot = view.board().card(Path::new("/demo/minibot")).unwrap();
        assert!(minibot.document.is_some(), "the new repository was fetched");
        // gh-x kept its status and stayed expanded.
        assert!(view.board().card(Path::new("/demo/gh-x")).unwrap().document.is_some());
        assert!(view.is_expanded(Path::new("/demo/gh-x")));
        // Its T3 lookup covers the new row too.
        assert!(view.thread(Path::new("/demo/minibot")).is_some());
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(("row", 1usize)).is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
async fn a_repository_removed_from_the_config_disappears(cx: &mut TestAppContext) {
    let config = ConfigDir::new("remove", &["/demo/gh-x", "/demo/codexbar"]);
    let (handle, view) = open_watching(cx, &config, &["/demo/gh-x", "/demo/codexbar"]);
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
    settle(cx, handle, &view).await;
    config.write(&["/demo/codexbar"]);
    after_poll(cx, handle, &view).await;
    assert_eq!(listed(cx, &view), ["codexbar"]);
}

#[gpui_kit::test]
async fn a_lookup_running_during_a_removal_does_not_bring_the_repository_back(cx: &mut TestAppContext) {
    let config = ConfigDir::new("removal-lookup", &["/demo/gh-x", "/demo/codexbar"]);
    let (handle, view) = open_watching(cx, &config, &["/demo/gh-x", "/demo/codexbar"]);
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
    settle(cx, handle, &view).await;
    // Start a lookup, then remove codexbar before it lands.
    cx.update(|cx| view.update(cx, |view, cx| view.find_threads(cx)));
    config.write(&["/demo/gh-x"]);
    cx.update(|cx| view.update(cx, |view, cx| view.check_config(cx)));
    after_poll(cx, handle, &view).await;
    cx.update(|cx| {
        let view = view.read(cx);
        assert!(view.thread(Path::new("/demo/codexbar")).is_none());
        assert!(view.thread(Path::new("/demo/gh-x")).is_some());
    });
}

#[gpui_kit::test]
async fn a_removed_repository_leaves_the_status_cache_at_once(cx: &mut TestAppContext) {
    let config = ConfigDir::new("cache-prune", &["/demo/gh-x", "/demo/codexbar"]);
    let (handle, view) = open_board(
        cx,
        vec![PathBuf::from("/demo/gh-x"), PathBuf::from("/demo/codexbar")],
        StatusCache::default(),
        Arc::new(demo::DemoSource),
        Options {
            config_dir: Some(config.0.clone()),
            cache_dir: Some(config.0.clone()),
            ..options()
        },
    );
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
    settle(cx, handle, &view).await;
    until(cx, |_| {
        StatusCache::load(&config.0).get(Path::new("/demo/codexbar")).is_some()
    });
    // Automatic refresh is off in this config: removing a repository alone must still prune the cache file.
    config.write(&["/demo/gh-x"]);
    after_poll(cx, handle, &view).await;
    until(cx, |_| {
        StatusCache::load(&config.0).get(Path::new("/demo/codexbar")).is_none()
    });
    let cache = StatusCache::load(&config.0);
    assert!(cache.get(Path::new("/demo/codexbar")).is_none());
    assert!(cache.get(Path::new("/demo/gh-x")).is_some());
}

#[gpui_kit::test]
async fn turning_automatic_refresh_off_stops_the_refresh_already_waiting(cx: &mut TestAppContext) {
    let config = ConfigDir::new("auto-off", &[]);
    config.write_text(
        "refresh_minutes = 1
repositories = [\"/demo/gh-x\"]
",
    );
    let source = Arc::new(Recording::default());
    let (handle, view) = open_board(
        cx,
        vec![PathBuf::from("/demo/gh-x")],
        StatusCache::default(),
        source.clone(),
        Options {
            config_dir: Some(config.0.clone()),
            auto_refresh: Some(Duration::from_secs(60)),
            ..options()
        },
    );
    cx.run_until_parked();
    // While the one-minute wait runs, the file turns automatic refresh off.
    config.write_text(
        "refresh_minutes = 0
repositories = [\"/demo/gh-x\"]
",
    );
    after_poll(cx, handle, &view).await;
    for _ in 0..30 {
        cx.executor().advance_clock(Duration::from_secs(5));
        cx.run_until_parked();
    }
    settle(cx, handle, &view).await;
    assert!(source.0.lock().unwrap().is_empty(), "no refresh at the old deadline");
}

#[gpui_kit::test]
async fn a_shorter_refresh_interval_applies_without_waiting_out_the_old_one(cx: &mut TestAppContext) {
    let config = ConfigDir::new("interval", &[]);
    config.write_text("refresh_minutes = 5\nrepositories = [\"/demo/gh-x\"]\n");
    let source = Arc::new(Recording::default());
    let (handle, view) = open_board(
        cx,
        vec![PathBuf::from("/demo/gh-x")],
        StatusCache::default(),
        source.clone(),
        Options {
            config_dir: Some(config.0.clone()),
            auto_refresh: Some(Duration::from_secs(300)),
            ..options()
        },
    );
    cx.run_until_parked();
    // During the five-minute wait the file asks for one minute.
    config.write_text("refresh_minutes = 1\nrepositories = [\"/demo/gh-x\"]\n");
    after_poll(cx, handle, &view).await;
    for _ in 0..14 {
        cx.executor().advance_clock(Duration::from_secs(5));
        cx.run_until_parked();
    }
    settle(cx, handle, &view).await;
    until(cx, |_| !source.0.lock().unwrap().is_empty());
    assert_eq!(
        source.0.lock().unwrap().len(),
        1,
        "refreshed on the new one-minute schedule"
    );
}

#[gpui_kit::test]
async fn a_momentarily_empty_config_does_not_empty_the_table(cx: &mut TestAppContext) {
    let config = ConfigDir::new("transient", &["/demo/gh-x", "/demo/codexbar"]);
    let (handle, view) = open_watching(cx, &config, &["/demo/gh-x", "/demo/codexbar"]);
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
    settle(cx, handle, &view).await;
    // An editor truncates the file and a poll catches it empty, which is valid TOML with no repositories.
    std::fs::write(config.0.join("config.toml"), "").unwrap();
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    // The save completes before the next poll: the empty read was never applied.
    config.write(&["/demo/gh-x", "/demo/codexbar", "/demo/minibot"]);
    after_poll(cx, handle, &view).await;
    assert_eq!(listed(cx, &view), ["gh-x", "codexbar", "minibot"]);
    assert!(cx.update(|cx| {
        view.read(cx)
            .board()
            .card(Path::new("/demo/gh-x"))
            .unwrap()
            .document
            .is_some()
    }));
}

#[gpui_kit::test]
async fn an_edit_saved_while_the_app_starts_is_not_lost(cx: &mut TestAppContext) {
    // The app read a two-repository config at startup; by the time the watcher starts, the file has a third.
    let config = ConfigDir::new("startup-race", &["/demo/gh-x", "/demo/codexbar", "/demo/minibot"]);
    let (handle, view) = open_watching(cx, &config, &["/demo/gh-x", "/demo/codexbar"]);
    after_poll(cx, handle, &view).await;
    assert_eq!(listed(cx, &view), ["gh-x", "codexbar", "minibot"]);
}

#[gpui_kit::test]
async fn refresh_all_reads_the_config_first(cx: &mut TestAppContext) {
    let config = ConfigDir::new("refresh", &["/demo/gh-x"]);
    let (handle, view) = open_watching(cx, &config, &["/demo/gh-x"]);
    config.write(&["/demo/gh-x", "/demo/codexbar"]);
    // F5 before the poll has run: the refresh still covers the new list.
    cx.update_window(handle, |_, window, cx| window.press("f5", cx))
        .unwrap();
    settle(cx, handle, &view).await;
    assert_eq!(listed(cx, &view), ["gh-x", "codexbar"]);
    assert!(cx.update(|cx| {
        view.read(cx)
            .board()
            .card(Path::new("/demo/codexbar"))
            .unwrap()
            .document
            .is_some()
    }));
}

#[gpui_kit::test]
async fn a_broken_config_keeps_the_table_and_says_so_until_it_is_fixed(cx: &mut TestAppContext) {
    let config = ConfigDir::new("broken", &["/demo/gh-x", "/demo/codexbar"]);
    let (handle, view) = open_watching(cx, &config, &["/demo/gh-x", "/demo/codexbar"]);
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
    settle(cx, handle, &view).await;
    assert!(!banner(cx, handle));
    config.write_text("repositories = [\"/demo/gh-x\"");
    after_poll(cx, handle, &view).await;
    // The last good list stays, and the banner names the file and says it will be read again.
    assert_eq!(listed(cx, &view), ["gh-x", "codexbar"]);
    assert!(banner(cx, handle));
    let problem = cx
        .update(|cx| view.read(cx).config_problem().map(str::to_owned))
        .unwrap();
    assert!(problem.contains("config.toml is not valid"), "{problem}");
    assert!(problem.contains("reads the file again when it is saved"), "{problem}");
    assert!(!problem.contains("restart"), "{problem}");
    // Fixed: the banner goes and the new list applies.
    config.write(&["/demo/codexbar"]);
    after_poll(cx, handle, &view).await;
    assert!(!banner(cx, handle));
    assert_eq!(listed(cx, &view), ["codexbar"]);
}

#[gpui_kit::test]
async fn an_unchanged_config_starts_no_extra_fetches(cx: &mut TestAppContext) {
    let config = ConfigDir::new("unchanged", &["/demo/gh-x"]);
    let source = Arc::new(Recording::default());
    let (handle, view) = open_board(
        cx,
        vec![PathBuf::from("/demo/gh-x")],
        StatusCache::default(),
        source.clone(),
        Options {
            config_dir: Some(config.0.clone()),
            ..options()
        },
    );
    for _ in 0..5 {
        after_poll(cx, handle, &view).await;
    }
    assert!(
        source.0.lock().unwrap().is_empty(),
        "polling an unchanged file fetches nothing"
    );
}

#[gpui_kit::test]
async fn priority_values_precede_prs_and_sort_without_expanding_rows(cx: &mut TestAppContext) {
    let (handle, view) = twelve(cx).await;
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(("priority", 0usize)).label(), Some("Priority P3"));
        assert_eq!(window.find(("priority", 2usize)).label(), Some("Priority: unknown"));
        let priority = window.find("sort-priority").bounds();
        let row = window.find(("priority", 0usize)).bounds();
        let prs = window.find(("prs", 0usize)).bounds();
        assert_eq!(priority.origin.x, row.origin.x);
        assert!(row.origin.x + row.size.width < prs.origin.x);
        window.click("sort-priority", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| view.read(cx).sort()), SortOrder::PriorityFirst);
    let order = cx.update(|cx| view.read(cx).row_order());
    assert_eq!(order[0], Path::new("/demo/set-it-free-loop"));
    assert_eq!(order[1], Path::new("/demo/hs-buddy"));
    assert_eq!(order[2], Path::new("/demo/gh-x"));
    assert_eq!(order[6], Path::new("/demo/minibot"));
    assert!(!cx.update(|cx| view.read(cx).is_expanded(Path::new("/demo/gh-x"))));
    cx.update_window(handle, |_, window, cx| window.click("sort-priority", cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| view.read(cx).sort()), SortOrder::PriorityLast);
    assert_eq!(cx.update(|cx| view.read(cx).row_order())[0], Path::new("/demo/gh-x"));
    cx.update_window(handle, |_, window, cx| window.click("sort-priority", cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| view.read(cx).row_order()), demo::checkouts());
    cx.update_window(handle, |_, window, cx| {
        window.click("sort-priority", cx);
        window.render_frame(cx);
        window.click("sort-updated", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| view.read(cx).sort()), SortOrder::NewestFirst);
}

#[gpui_kit::test]
async fn priority_width_survives_a_restart_and_dragging_does_not_sort(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("dashboard-priority-width-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (handle, view) = open_board(
        cx,
        four(),
        StatusCache::default(),
        Arc::new(demo::DemoSource),
        Options {
            ui_state_dir: Some(dir.clone()),
            ..options()
        },
    );
    drag_border(cx, handle, Column::Priority, 32.);
    let width = Column::Priority.default_width() + 2.;
    assert_eq!(cx.update(|cx| view.read(cx).columns()).get(Column::Priority), width);
    assert_eq!(width_of(cx, handle, "sort-priority"), px(width * 16.));
    assert_eq!(width_of(cx, handle, ("priority", 0usize)), px(width * 16.));
    assert_eq!(cx.update(|cx| view.read(cx).sort()), SortOrder::Config);
    let saved = dashboard_core::ui_state::UiState::load(&dir).columns;
    assert_eq!(saved.get(Column::Priority), width);
    let (restored, _) = open_board(
        cx,
        four(),
        StatusCache::default(),
        Arc::new(demo::DemoSource),
        Options {
            columns: saved,
            ..options()
        },
    );
    assert_eq!(width_of(cx, restored, "sort-priority"), px(width * 16.));
    cx.update_window(handle, |_, window, cx| {
        window.double_click(("resize", Column::Priority as usize), cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        cx.update(|cx| view.read(cx).columns()).get(Column::Priority),
        Column::Priority.default_width()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui_kit::test]
async fn a_blocked_priority_source_never_holds_up_status_refreshes(cx: &mut TestAppContext) {
    use dashboard_core::priority::{Priorities, PrioritySource};
    struct Gated {
        released: std::sync::Mutex<bool>,
        ready: std::sync::Condvar,
        calls: std::sync::atomic::AtomicUsize,
    }
    impl PrioritySource for Gated {
        fn fetch(&self) -> Priorities {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let released = self.released.lock().unwrap();
            let _released = self.ready.wait_while(released, |released| !*released).unwrap();
            Priorities::default()
        }
    }
    struct Release(Arc<Gated>);
    impl Drop for Release {
        fn drop(&mut self) {
            *self.0.released.lock().unwrap() = true;
            self.0.ready.notify_all();
        }
    }
    let source = Arc::new(Gated {
        released: std::sync::Mutex::new(false),
        ready: std::sync::Condvar::new(),
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    let release = Release(Arc::clone(&source));
    let (handle, view) = open_board(
        cx,
        four(),
        StatusCache::default(),
        Arc::new(demo::DemoSource),
        Options {
            priorities: source.clone(),
            ..options()
        },
    );
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.refresh_all(cx);
            view.refresh_all(cx);
        })
    });
    for _ in 0..100 {
        std::thread::sleep(Duration::from_millis(2));
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        if !cx.update(|cx| view.read(cx).board().is_refreshing()) {
            break;
        }
    }
    assert!(!cx.update(|cx| view.read(cx).board().is_refreshing()));
    assert!(cx.update(|cx| view.read(cx).priorities_pending()));
    assert_eq!(source.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(("priority", 0usize)).label(), Some("Priority: unknown"));
        assert_eq!(window.find(("prs", 0usize)).label(), Some("0 open pull requests"));
    })
    .unwrap();
    drop(release);
    settle(cx, handle, &view).await;
    assert!(!cx.update(|cx| view.read(cx).priorities_pending()));
}

#[test]
fn unavailable_sections_with_rows_keep_their_counts() {
    let document = fixtures::document(fixtures::ENRICHMENT_ERRORS);
    for (section, count) in [
        ("pullRequests.open", document.pull_requests.open.len()),
        ("issues", document.issues.len()),
        ("pullRequests.merged", document.pull_requests.merged.len()),
        ("workflowRuns", document.workflow_runs.len()),
    ] {
        assert!(count > 0);
        assert_eq!(section_count(&document, section, count), count.to_string(), "{section}");
        assert_eq!(section_count(&document, section, 0), "?", "empty {section}");
    }
}

#[gpui_kit::test]
async fn enrichment_errors_keep_six_prs_counts_diagnostics_and_clickable_rows(cx: &mut TestAppContext) {
    let (handle, view) = open_board(
        cx,
        vec!["/demo/project-manager".into()],
        StatusCache::default(),
        Arc::new(demo::DemoSource),
        options(),
    );
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
    settle(cx, handle, &view).await;
    assert_eq!(overall(cx, &view), [Overall::Attention]);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(("prs", 0usize)).label(), Some("6 open pull requests"));
        assert_eq!(window.find(("issues", 0usize)).label(), Some("2 open issues"));
        window.click(("expand", 0usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let document = fixtures::document(fixtures::ENRICHMENT_ERRORS);
        for (title, count, reason) in [
            (
                "Open PRs",
                6,
                "truncated supplemental connections for pull request(s) 32",
            ),
            ("Open issues", 2, "issue enrichment timed out"),
            ("Recently merged", 1, "merged pull request enrichment timed out"),
            ("CI runs", 2, "workflow run enrichment timed out"),
        ] {
            assert_eq!(
                window.find(format!("heading-{title}")).label(),
                Some(format!("{title} ({count})").as_str())
            );
            assert_eq!(
                window.find(format!("unavailable-{title}")).label(),
                Some(format!("Unavailable: {reason}").as_str())
            );
        }
        for pr in &document.pull_requests.open {
            assert!(window.try_find(format!("pr-0-{}", pr.number)).is_some());
        }
        for issue in &document.issues {
            assert!(window.try_find(format!("issue-0-{}", issue.number)).is_some());
        }
        for pr in &document.pull_requests.merged {
            assert!(window.try_find(format!("merged-0-{}", pr.number)).is_some());
        }
        for run in &document.workflow_runs {
            assert!(window.try_find(format!("run-0-{}", run.id)).is_some());
        }
        let note = window.find("unavailable-Open PRs").bounds();
        let row = window.find("pr-0-27").bounds();
        assert!(note.origin.y + note.size.height <= row.origin.y);
        window.click("pr-0-32", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        cx.opened_url(),
        Some("https://github.com/hemsoft-dev/project-manager/pull/32".to_owned())
    );
}

/// Enrichment can fail without supplying diagnostic text; loaded rows are still useful.
struct EnrichmentWithoutReason;

impl StatusSource for EnrichmentWithoutReason {
    fn fetch(&self, _: &Path, _: bool) -> Result<StatusDocument, FetchError> {
        let mut document = fixtures::document(fixtures::ENRICHMENT_ERRORS);
        for section in &mut document.sections {
            section.reason.clear();
        }
        Ok(document)
    }
}

#[gpui_kit::test]
async fn unavailable_rows_survive_even_without_diagnostic_text(cx: &mut TestAppContext) {
    let (handle, view) = open_board(
        cx,
        vec!["/demo/project-manager".into()],
        StatusCache::default(),
        Arc::new(EnrichmentWithoutReason),
        options(),
    );
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
    settle(cx, handle, &view).await;
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(("prs", 0usize)).label(), Some("6 open pull requests"));
        window.click(("expand", 0usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        for number in 27..=32 {
            assert!(window.try_find(format!("pr-0-{number}")).is_some());
        }
        assert_eq!(window.find("heading-Open PRs").label(), Some("Open PRs (6)"));
    })
    .unwrap();
}

#[gpui_kit::test]
async fn hosted_details_put_blocked_and_waiting_first_then_open_work_in_plain_words(cx: &mut TestAppContext) {
    let checkout = PathBuf::from("/demo/codexbar");
    let (handle, view) = open_board(
        cx,
        vec![checkout.clone()],
        StatusCache::default(),
        Arc::new(demo::HostedSource),
        options(),
    );
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
    settle(cx, handle, &view).await;
    assert_eq!(overall(cx, &view), [Overall::Blocked]);
    cx.update(|cx| view.update(cx, |view, cx| view.toggle_expanded(&checkout, cx)));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // Under the name: both ages in words, never seconds.
        assert_eq!(
            window.find(("pmd-age", 0usize)).label(),
            Some("checked 12m ago · changed 5h 23m ago")
        );
        // Blocked at the top, then waiting on Franz, then open PRs and issues, then In progress. Ready items the
        // lists already show (#231) aren't repeated; the one they don't (#31) is.
        let headings = [
            "Blocked (1)",
            "Waiting on Franz (1)",
            "Open PRs (2)",
            "Open issues (2)",
            "In progress (1)",
            "Other work not started (1)",
            "Recently merged (1)",
            "CI runs (2)",
        ];
        let tops: Vec<_> = headings
            .iter()
            .map(|heading| {
                let title = heading.rsplit_once(" (").unwrap().0;
                let found = window.find(format!("heading-{title}"));
                assert_eq!(found.label(), Some(*heading));
                found.bounds().top()
            })
            .collect();
        assert!(tops.is_sorted(), "{tops:?}");
        assert!(window.try_find("heading-Ready").is_none());
        // Where the data came from, last, in local time with no UTC timestamp; local health is one line.
        let about = window.find(("pmd-freshness", 0usize));
        let text = about.label().unwrap();
        assert!(
            text.starts_with("Status changed ") && text.contains("(5h 23m ago)"),
            "{text}"
        );
        assert!(
            text.contains("pmd last checked ") && text.contains("Board as of "),
            "{text}"
        );
        assert!(
            !text.contains('Z') && !text.contains("UTC") && !text.contains("2026-"),
            "{text}"
        );
        assert!(about.bounds().top() > *tops.last().unwrap());
        assert!(window.try_find(("local-health", 0usize)).is_some());
        assert!(window.try_find("health-Working tree").is_none());
        window.click("work-0-fixture-stable-id", cx);
    })
    .unwrap();
    assert_eq!(
        cx.opened_url(),
        Some("https://github.com/hemsoft-dev/codexbar/issues/31".into())
    );
}

#[gpui_kit::test]
async fn hosted_dots_show_blocked_waiting_failing_checks_and_ok(cx: &mut TestAppContext) {
    let checkouts: Vec<PathBuf> = ["codexbar", "hs-buddy", "summarist", "gh-x", "minibot"]
        .iter()
        .map(|name| PathBuf::from("/demo").join(name))
        .collect();
    let (handle, view) = open_board(
        cx,
        checkouts,
        StatusCache::default(),
        Arc::new(demo::HostedSource),
        options(),
    );
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
    settle(cx, handle, &view).await;
    // Old but unchanged status (5h) doesn't grey a dot: only pmd's work items and checks color it.
    assert_eq!(
        overall(cx, &view),
        [
            Overall::Blocked,
            Overall::WaitingOnFranz,
            Overall::ChecksFailing,
            Overall::Clean,
            Overall::Unavailable
        ]
    );
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("state-0-Blocked").label(), Some("State: Blocked"));
        assert_eq!(
            window.find("state-1-WaitingOnFranz").label(),
            Some("State: Waiting on Franz")
        );
        assert_eq!(
            window.find("state-2-ChecksFailing").label(),
            Some("State: Checks failing")
        );
        assert_eq!(window.find("state-3-Clean").label(), Some("State: OK"));
        assert_eq!(
            window.find("state-4-Unavailable").label(),
            Some("State: Board unavailable")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
async fn cached_hosted_status_keeps_age_and_last_error_then_recovers(cx: &mut TestAppContext) {
    let checkout = PathBuf::from("/demo/codexbar");
    let mut document = demo::HostedSource.fetch(&checkout, false).unwrap();
    document.hosted.as_mut().unwrap().last_error = Some(dashboard_core::pmd::LastError {
        message: "upstream unavailable".into(),
        at: "2026-10-21T06:25:00Z".into(),
    });
    let mut cache = StatusCache::default();
    cache.insert(checkout.clone(), document);
    let (handle, view) = open_board(
        cx,
        vec![checkout.clone()],
        cache,
        Arc::new(demo::HostedSource),
        options(),
    );
    assert_eq!(overall(cx, &view), [Overall::Failed]);
    cx.update(|cx| view.update(cx, |view, cx| view.toggle_expanded(&checkout, cx)));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .find(("row-error", 0usize))
                .label()
                .unwrap()
                .contains("upstream unavailable")
        );
        assert!(window.try_find(("pmd-freshness", 0usize)).is_some());
    })
    .unwrap();
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_one(&checkout, cx)));
    settle(cx, handle, &view).await;
    assert_eq!(overall(cx, &view), [Overall::Blocked]);
    assert!(cx.update(|cx| view.read(cx).board().cards()[0].error.is_none()));
}

#[path = "../../dashboard-core/tests/support/http.rs"]
mod pmd_http;

const OLD_TOKEN: &str = "fixture-read-token";
const NEW_TOKEN: &str = "replacement-read-token";
const OLD_STAMP: &str = "2026-10-25T20:00:00Z";
const NEW_STAMP: &str = "2026-10-25T20:10:00Z";

fn pmd_response(path: &str, token: &str) -> pmd_http::Reply {
    let (stamp, label, priority) = if token == OLD_TOKEN {
        (OLD_STAMP, "old", 1)
    } else {
        assert_eq!(token, NEW_TOKEN);
        (NEW_STAMP, "new", 5)
    };
    let item = r#"{"id":"stable-work-id","project":"gh-x","kind":"issue","number":31,"title":"LABEL work","url":"https://github.com/hemsoft-dev/gh-x/issues/31","status":"ready"}"#.replace("LABEL", label);
    let body = match path {
        "/v1/projects" => r#"{"projects":[{"slug":"gh-x","repo":"hemsoft-dev/gh-x","priority":PRIORITY,"tracked":true}]}"#.replace("PRIORITY", &priority.to_string()),
        "/v1/projects/gh-x/status" => format!(r#"{{"project":"gh-x","generatedAt":"{stamp}","lastError":null,"status":{}}}"#, fixtures::CLEAN),
        "/v1/board?project=gh-x" => r#"{"generatedAt":"STAMP","columns":[{"status":"ready","title":"Ready","items":[ITEM],"truncated":false},{"status":"in_progress","title":"In progress","items":[],"truncated":false},{"status":"waiting_on_franz","title":"Waiting on Franz","items":[],"truncated":false},{"status":"blocked","title":"Blocked","items":[],"truncated":false}]}"#.replace("STAMP",stamp).replace("ITEM",&item),
        "/v1/work?project=gh-x&limit=500" => format!(r#"{{"items":[{item}],"nextCursor":null}}"#),
        "/healthz" => r#"{"sweepAgeSeconds":60,"webhookAgeSeconds":null,"githubLow":false}"#.to_owned(),
        _ => panic!("unexpected fixture path {path}"),
    };
    pmd_http::Reply { code: 200, body }
}

struct LivePmd {
    root: ConfigDir,
    config: dashboard_core::config::Config,
    checkout: PathBuf,
    handle: AnyWindowHandle,
    view: Entity<BoardView>,
}
impl LivePmd {
    fn new(cx: &mut TestAppContext, server: &pmd_http::Server, name: &str) -> Self {
        let root = ConfigDir::new(name, &[]);
        let checkout = root.0.join("checkout");
        std::fs::create_dir_all(checkout.join(".git")).unwrap();
        std::fs::write(
            checkout.join(".git/config"),
            "[remote \"origin\"]\nurl = git@github.com:hemsoft-dev/gh-x.git\n",
        )
        .unwrap();
        std::fs::write(root.0.join("old-token"), OLD_TOKEN).unwrap();
        std::fs::write(root.0.join("new-token"), NEW_TOKEN).unwrap();
        let config = dashboard_core::config::Config {
            repositories: vec![checkout.clone()],
            refresh_minutes: 0,
            parallelism: 2,
            pmd_url: server.url.clone(),
            pmd_token_path: "old-token".into(),
            ..Default::default()
        };
        root.write_text(&config.to_toml());
        let pmd = dashboard_core::priority::Pmd::from_config(&config, &root.0);
        let (handle, view) = open_board(
            cx,
            vec![checkout.clone()],
            StatusCache::default(),
            Arc::new(pmd.clone()),
            Options {
                config_dir: Some(root.0.clone()),
                cache_dir: Some(root.0.clone()),
                priorities: Arc::new(pmd.clone()),
                pmd: Some(pmd),
                ..options()
            },
        );
        cx.update(|cx| view.update(cx, |view, cx| view.refresh_all(cx)));
        let live = Self {
            root,
            config,
            checkout,
            handle,
            view,
        };
        live.wait(cx);
        cx.update(|cx| {
            live.view
                .update(cx, |view, cx| view.toggle_expanded(&live.checkout, cx))
        });
        assert_eq!(live.document(cx).hosted.as_ref().unwrap().generated_at, OLD_STAMP);
        live
    }
    fn wait(&self, cx: &mut TestAppContext) {
        until(cx, |cx| {
            cx.update(|cx| {
                !self.view.read(cx).board().is_refreshing()
                    && !self.view.read(cx).priorities_pending()
                    && self.view.read(cx).pending_refreshes() == 0
            })
        });
    }
    fn document(&self, cx: &mut TestAppContext) -> StatusDocument {
        cx.update(|cx| {
            self.view.read(cx).board().cards()[0]
                .document
                .clone()
                .expect("last good status")
        })
    }
    fn reload(&self, cx: &mut TestAppContext) {
        self.root.write_text(&self.config.to_toml());
        cx.update(|cx| self.view.update(cx, |view, cx| view.refresh_all(cx)));
    }
    fn assert_retained(&self, cx: &mut TestAppContext, original: &StatusDocument) {
        assert_eq!(&self.document(cx), original);
        assert_eq!(
            cx.update(|cx| self.view.read(cx).cached(&self.checkout).cloned())
                .as_ref(),
            Some(original)
        );
        assert_eq!(StatusCache::load(&self.root.0).get(&self.checkout), Some(original));
    }
}

fn token_failure_keeps_last_good_data(cx: &mut TestAppContext, code: u16, missing: bool, name: &str) {
    let server = pmd_http::Server::with_auth(move |path, token| {
        if token == NEW_TOKEN {
            pmd_http::Reply {
                code,
                body: String::new(),
            }
        } else {
            pmd_response(path, token)
        }
    });
    let mut live = LivePmd::new(cx, &server, name);
    let original = live.document(cx);
    live.config.pmd_token_path = if missing {
        "missing-token".into()
    } else {
        "new-token".into()
    };
    live.reload(cx);
    live.assert_retained(cx, &original);
    live.wait(cx);
    live.assert_retained(cx, &original);
    let error = if missing {
        FetchError::MissingToken
    } else if code == 401 {
        FetchError::Unauthorized
    } else {
        FetchError::Http(code)
    };
    cx.update(|cx| {
        let board = live.view.read(cx).board();
        assert_eq!(board.cards()[0].error, Some(error));
        assert_eq!(
            board.priority(&board.cards()[0]),
            Some(1),
            "retain last good priority on credential failure"
        );
    });
    assert!(!server.requests.lock().unwrap().is_empty());
    cx.update_window(live.handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(("prs", 0usize)).is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
fn token_only_missing_token_preserves_status_age_board_work_cache_and_priority(cx: &mut TestAppContext) {
    token_failure_keeps_last_good_data(cx, 200, true, "pmd-token-missing");
}
#[gpui_kit::test]
fn token_only_unauthorized_preserves_status_age_board_work_cache_and_priority(cx: &mut TestAppContext) {
    token_failure_keeps_last_good_data(cx, 401, false, "pmd-token-401");
}
#[gpui_kit::test]
fn token_only_outage_preserves_status_age_board_work_cache_and_priority(cx: &mut TestAppContext) {
    token_failure_keeps_last_good_data(cx, 503, false, "pmd-token-outage");
}
#[gpui_kit::test]
fn token_only_success_replaces_retained_status_board_work_and_priority(cx: &mut TestAppContext) {
    let server = pmd_http::Server::with_auth(pmd_response);
    let mut live = LivePmd::new(cx, &server, "pmd-token-success");
    let original = live.document(cx);
    live.config.pmd_token_path = "new-token".into();
    live.reload(cx);
    live.assert_retained(cx, &original);
    live.wait(cx);
    let document = live.document(cx);
    let hosted = document.hosted.as_ref().unwrap();
    assert_eq!(hosted.generated_at, NEW_STAMP);
    assert_eq!(hosted.board.as_ref().unwrap().columns[0].items[0].title, "new work");
    assert_eq!(hosted.work[0].title, "new work");
    cx.update(|cx| {
        let board = live.view.read(cx).board();
        assert!(board.cards()[0].error.is_none());
        assert_eq!(board.priority(&board.cards()[0]), Some(5));
    });
    live.assert_retained(cx, &document);
}
#[gpui_kit::test]
fn changed_pmd_url_still_discards_status_board_work_cache_and_priority(cx: &mut TestAppContext) {
    let old = pmd_http::Server::new(|path| pmd_response(path, OLD_TOKEN));
    let new = pmd_http::Server::new(|_| pmd_http::Reply {
        code: 503,
        body: String::new(),
    });
    let mut live = LivePmd::new(cx, &old, "pmd-url-change");
    live.config.pmd_url = new.url.clone();
    live.reload(cx);
    cx.update(|cx| {
        let view = live.view.read(cx);
        let board = view.board();
        assert!(board.cards()[0].document.is_none());
        assert!(view.cached(&live.checkout).is_none());
        assert_eq!(board.priority(&board.cards()[0]), None);
    });
    live.wait(cx);
    cx.update(|cx| {
        let board = live.view.read(cx).board();
        assert!(board.cards()[0].document.is_none());
        assert_eq!(board.cards()[0].error, Some(FetchError::Http(503)));
    });
    assert!(StatusCache::load(&live.root.0).is_empty());
}

#[gpui_kit::test]
fn token_only_change_rejects_delayed_old_token_status_after_new_success(cx: &mut TestAppContext) {
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    let delay = Arc::new(AtomicBool::new(false));
    let hold = delay.clone();
    let entered = Arc::new(AtomicBool::new(false));
    let seen = entered.clone();
    let (release, wait) = mpsc::channel();
    let wait = Mutex::new(wait);
    let server = pmd_http::Server::with_auth(move |path, token| {
        if token == OLD_TOKEN && path.ends_with("/status") && hold.load(Ordering::SeqCst) {
            seen.store(true, Ordering::SeqCst);
            let _ = wait.lock().unwrap().recv_timeout(Duration::from_secs(5));
        }
        pmd_response(path, token)
    });
    let mut live = LivePmd::new(cx, &server, "pmd-token-delayed");
    let original = live.document(cx);
    let generation = cx.update(|cx| live.view.read(cx).board().cards()[0].generation);
    delay.store(true, Ordering::SeqCst);
    cx.update(|cx| live.view.update(cx, |view, cx| view.refresh_one(&live.checkout, cx)));
    until(cx, |_| entered.load(Ordering::SeqCst));
    live.config.pmd_token_path = "new-token".into();
    live.reload(cx);
    live.assert_retained(cx, &original);
    assert_ne!(
        cx.update(|cx| live.view.read(cx).board().cards()[0].generation),
        generation
    );
    until(cx, |cx| {
        cx.update(|cx| {
            live.view.read(cx).board().cards()[0]
                .document
                .as_ref()
                .and_then(|doc| doc.hosted.as_ref())
                .is_some_and(|hosted| hosted.generated_at == NEW_STAMP)
        })
    });
    release.send(()).unwrap();
    live.wait(cx);
    let current = live.document(cx);
    assert_eq!(current.hosted.as_ref().unwrap().generated_at, NEW_STAMP);
    assert_eq!(current.hosted.as_ref().unwrap().work[0].title, "new work");
    assert!(cx.update(|cx| live.view.read(cx).board().cards()[0].error.is_none()));
    live.assert_retained(cx, &current);
}

#[gpui_kit::test]
fn token_only_successful_untiered_priority_response_clears_previous_priority(cx: &mut TestAppContext) {
    let server = pmd_http::Server::with_auth(|path, token| {
        let mut response = pmd_response(path, token);
        if token == NEW_TOKEN && path == "/v1/projects" {
            response.body = response.body.replace("\"priority\":5", "\"priority\":null");
        }
        response
    });
    let mut live = LivePmd::new(cx, &server, "pmd-priority-untiered");
    live.config.pmd_token_path = "new-token".into();
    live.reload(cx);
    live.wait(cx);
    assert_eq!(live.document(cx).hosted.as_ref().unwrap().generated_at, NEW_STAMP);
    cx.update(|cx| {
        let board = live.view.read(cx).board();
        assert_eq!(board.priority(&board.cards()[0]), None);
    });
}

#[gpui_kit::test]
fn editing_pmd_write_token_path_turns_writes_on_and_off(cx: &mut TestAppContext) {
    use dashboard_core::write::Access;
    let server = pmd_http::Server::with_auth(pmd_response);
    let mut live = LivePmd::new(cx, &server, "pmd-write-path");
    let manage = cx.update(|cx| live.view.read(cx).manage().clone());
    assert_eq!(cx.update(|cx| manage.read(cx).access().clone()), Access::Off);
    live.config.pmd_write_token_path = Some("pmd-write-token".into());
    live.reload(cx);
    live.wait(cx);
    assert_eq!(cx.update(|cx| manage.read(cx).access().clone()), Access::Unchecked);
    // The token never goes in config.toml; only its path does.
    assert!(
        live.config
            .to_toml()
            .contains("pmd_write_token_path = \"pmd-write-token\"")
    );
    live.config.pmd_write_token_path = None;
    live.reload(cx);
    live.wait(cx);
    assert_eq!(cx.update(|cx| manage.read(cx).access().clone()), Access::Off);
}
