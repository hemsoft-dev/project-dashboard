//! Screenshots for pull requests, rendered offscreen by GPUI's Linux headless renderer (wgpu), so no window ever appears
//! on a desktop. Windows has no headless renderer in GPUI.
//!
//! `CAPTURE_DIR=<absolute folder> cargo test -p dashboard-app captures -- --ignored` writes one PNG per state. Cargo
//! runs the test from this crate's folder, so a relative folder lands under `crates/dashboard-app`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dashboard_core::cache::StatusCache;
use dashboard_core::scheduler::Outcome;
use dashboard_core::source::{FetchError, StatusSource};
use dashboard_core::status::StatusDocument;
use gpui_kit::component::Root;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, HeadlessAppContext, px, size};

use crate::board_view::{BoardView, Options};
use crate::{demo, theme};

fn save(cx: &mut HeadlessAppContext, handle: AnyWindowHandle, dir: &Path, name: &str) {
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
    let image = cx
        .capture_screenshot(handle)
        .expect("the headless renderer draws the window");
    image.save(dir.join(format!("{name}.png"))).unwrap();
}

fn options() -> Options {
    Options {
        cache_dir: None,
        parallelism: 4,
        auto_refresh: None,
        config_problem: None,
        config_path: Some(PathBuf::from("~/.config/project-dashboard/config.toml")),
        now: Some(demo::fixture_now()),
    }
}

fn open(cx: &mut HeadlessAppContext, checkouts: Vec<PathBuf>, height: f32) -> (AnyWindowHandle, Entity<BoardView>) {
    let mut view = None;
    let handle = cx
        .open_window(size(px(1100.), px(height)), |window, cx| {
            let board = cx.new(|cx| {
                BoardView::new(
                    &checkouts,
                    StatusCache::default(),
                    Arc::new(demo::DemoSource),
                    options(),
                    window,
                    cx,
                )
            });
            view = Some(board.clone());
            cx.new(|cx| Root::new(board, window, cx))
        })
        .unwrap();
    (handle.into(), view.unwrap())
}

/// Shows each checkout's result from `source` as a refresh would, without the scheduler's threads.
fn fill(cx: &mut HeadlessAppContext, view: &Entity<BoardView>, source: &dyn StatusSource) {
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            for checkout in view.board().checkouts() {
                let result = source.fetch(&checkout, false);
                view.apply(Outcome { checkout, result }, cx);
            }
        })
    });
    cx.run_until_parked();
}

#[test]
#[ignore = "writes screenshots; run with CAPTURE_DIR set"]
fn captures() {
    let Some(out) = std::env::var_os("CAPTURE_DIR").map(PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&out).unwrap();
    let text_system = gpui_kit::platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(text_system, Arc::new(gpui_kit::assets::Assets), || {
        gpui_kit::platform::current_headless_renderer()
    });
    cx.update(|cx| {
        gpui_kit::init(cx);
        theme::init(cx);
    });

    // The collapsed table, twelve rows: clean, attention, a partly unavailable repository and a failing one first.
    let (board, view) = open(&mut cx, demo::checkouts(), 560.);
    fill(&mut cx, &view, &demo::DemoSource);
    save(&mut cx, board, &out, "table");

    // The same table sorted by last activity, newest first.
    cx.update(|cx| view.update(cx, |view, cx| view.toggle_sort(cx)));
    cx.run_until_parked();
    save(&mut cx, board, &out, "table-sorted");

    // The table with codexbar expanded: its health rows and its PR, issue, merged-PR and workflow-run tables.
    let (expanded, expanded_view) = open(&mut cx, demo::checkouts(), 980.);
    fill(&mut cx, &expanded_view, &demo::DemoSource);
    cx.update(|cx| expanded_view.update(cx, |view, cx| view.toggle_expanded(Path::new("/demo/codexbar"), cx)));
    cx.run_until_parked();
    save(&mut cx, expanded, &out, "expanded-row");

    // The failing row expanded: its error in place of the last activity and again in its details.
    let (failed, failed_view) = open(&mut cx, demo::checkouts(), 560.);
    fill(&mut cx, &failed_view, &demo::DemoSource);
    cx.update(|cx| failed_view.update(cx, |view, cx| view.toggle_expanded(Path::new("/demo/weather-cli"), cx)));
    cx.run_until_parked();
    save(&mut cx, failed, &out, "error-row");

    // Frames for the expand-and-collapse recording: codexbar opens, hs-buddy opens too, then codexbar closes.
    let (sequence, sequence_view) = open(&mut cx, demo::checkouts(), 820.);
    fill(&mut cx, &sequence_view, &demo::DemoSource);
    save(&mut cx, sequence, &out, "expand-1");
    for (frame, checkout) in [(2, "/demo/codexbar"), (3, "/demo/hs-buddy"), (4, "/demo/codexbar")] {
        cx.update(|cx| sequence_view.update(cx, |view, cx| view.toggle_expanded(Path::new(checkout), cx)));
        cx.run_until_parked();
        save(&mut cx, sequence, &out, &format!("expand-{frame}"));
    }

    // A partly unavailable repository, expanded: the sections gh-x couldn't load say so instead of "None".
    let (partial, partial_view) = open(&mut cx, vec![PathBuf::from("/demo/minibot")], 520.);
    fill(&mut cx, &partial_view, &demo::DemoSource);
    cx.update(|cx| partial_view.update(cx, |view, cx| view.toggle_expanded(Path::new("/demo/minibot"), cx)));
    cx.run_until_parked();
    save(&mut cx, partial, &out, "expanded-unavailable");

    // Without gh: one setup message instead of a failure on every card.
    struct NoGh;
    impl StatusSource for NoGh {
        fn fetch(&self, _: &Path, _: bool) -> Result<StatusDocument, FetchError> {
            Err(FetchError::GhMissing)
        }
    }
    let (setup, setup_view) = open(&mut cx, demo::checkouts(), 520.);
    fill(&mut cx, &setup_view, &NoGh);
    save(&mut cx, setup, &out, "setup-missing-gh");

    let (empty, _) = open(&mut cx, vec![], 520.);
    cx.run_until_parked();
    save(&mut cx, empty, &out, "empty-board");
}
