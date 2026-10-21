//! Headless UI tests: the real board view in an in-process test window, with the fixture-backed demo source. Nothing
//! here opens a desktop window, runs gh, sends OS input or changes focus outside the test.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use dashboard_core::cache::StatusCache;
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
fn four() -> Vec<PathBuf> {
    demo::checkouts().into_iter().take(4).collect()
}

fn options() -> Options {
    Options {
        cache_dir: None,
        parallelism: 4,
        auto_refresh: None,
        config_problem: None,
        config_path: None,
        now: Some(demo::fixture_now()),
    }
}

fn open_board(
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
async fn refreshing_one_card_bypasses_the_gh_x_cache_and_refresh_all_does_not(cx: &mut TestAppContext) {
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
async fn a_missing_gh_shows_one_setup_message(cx: &mut TestAppContext) {
    struct NoGh;
    impl StatusSource for NoGh {
        fn fetch(&self, _: &Path, _: bool) -> Result<StatusDocument, FetchError> {
            Err(FetchError::GhMissing)
        }
    }
    let (handle, view) = open_board(cx, four(), StatusCache::default(), Arc::new(NoGh), options());
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
        assert!(window.try_find("unavailable-Open pull requests").is_some());
        assert!(window.try_find("unavailable-Open issues").is_some());
        // The merged list and runs loaded (empty), so they don't claim to be unavailable.
        assert!(window.try_find("unavailable-Recently merged").is_none());
        assert!(window.try_find("unavailable-Workflow runs").is_none());
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
async fn the_table_has_one_row_per_repository_with_counts_and_last_activity(cx: &mut TestAppContext) {
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
    assert_eq!(
        text_of(cx, handle, ("activity", 7)),
        "Last activity: Issue #231 · 14m ago, Show the tray icon on Linux"
    );
    let activity = cx.update(|cx| {
        view.read(cx)
            .board()
            .cards()
            .iter()
            .map(|card| {
                card.last_activity()
                    .map(|activity| activity.summary(demo::fixture_now()))
            })
            .collect::<Vec<_>>()
    });
    assert_eq!(activity[1].as_deref(), Some("CI run · 3m ago"));
    assert_eq!(activity[4].as_deref(), Some("PR #232 · 3m ago"));
    assert_eq!(activity[7].as_deref(), Some("Issue #231 · 14m ago"));
    assert_eq!(activity[2], None);
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
async fn the_last_activity_header_sorts_newest_first_and_back_to_config_order(cx: &mut TestAppContext) {
    let (handle, view) = twelve(cx).await;
    let names = |cx: &mut TestAppContext| {
        cx.update(|cx| view.read(cx).row_order())
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(cx.update(|cx| view.read(cx).sort()), SortOrder::Config);
    assert_eq!(names(cx)[..4], ["gh-x", "codexbar", "minibot", "weather-cli"]);
    cx.update_window(handle, |_, window, cx| window.click("sort-activity", cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| view.read(cx).sort()), SortOrder::LastActivity);
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
    cx.update_window(handle, |_, window, cx| window.click("sort-activity", cx))
        .unwrap();
    cx.run_until_parked();
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
async fn at_the_minimum_width_every_row_keeps_its_refresh_button_in_view(cx: &mut TestAppContext) {
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
        for ix in 0..12usize {
            let bounds = window.find(("refresh", ix)).bounds();
            assert!(
                bounds.origin.x + bounds.size.width <= px(480.),
                "row {ix}: refresh button ends at {:?}",
                bounds.origin.x + bounds.size.width
            );
        }
        // The name keeps room to read, rather than collapsing to nothing.
        // Every last-activity summary ends before its row's refresh button.
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
        for ix in 0..12usize {
            let name = window.find(("name", ix)).bounds();
            // NAME_MIN is 7 rem: 112 px at the 16 px base.
            assert!(
                name.size.width >= px(112.),
                "row {ix}: name is {:?} wide",
                name.size.width
            );
        }
        // The state is named for screen readers, not only colored.
        assert_eq!(window.find("state-3-Failed").label(), Some("State: Failed"));
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
    assert!(!dir.join(dashboard_core::zoom::STATE_FILE).exists());
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(dashboard_core::zoom::UiState::load(&dir).zoom, 1.3);
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
async fn at_200_percent_the_table_still_fits_its_columns(cx: &mut TestAppContext) {
    let (handle, _) = twelve(cx).await;
    cx.update(|cx| zoom::set_level(2.0, cx));
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
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
                window.find(("name", ix)).bounds().size.width > px(150.),
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
        assert!(window.find(("name", 0usize)).bounds().size.width >= px(7. * 32.));
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
