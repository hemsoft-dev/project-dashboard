//! Screenshots for pull requests, rendered offscreen by GPUI's Linux headless renderer (wgpu), so no window ever appears
//! on a desktop. Windows has no headless renderer in GPUI.
//!
//! `CAPTURE_DIR=<folder> cargo test -p dashboard-app captures -- --ignored` writes one PNG per state.

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

    // The populated board: clean, attention, a partly unavailable repository and a failing one.
    let (board, view) = open(&mut cx, demo::checkouts(), 880.);
    fill(&mut cx, &view, &demo::DemoSource);
    save(&mut cx, board, &out, "board");

    // The same board with codexbar expanded: its PR, issue, merged-PR and workflow-run tables.
    let (expanded, expanded_view) = open(&mut cx, vec![PathBuf::from("/demo/codexbar")], 720.);
    fill(&mut cx, &expanded_view, &demo::DemoSource);
    cx.update(|cx| expanded_view.update(cx, |view, cx| view.toggle_expanded(Path::new("/demo/codexbar"), cx)));
    cx.run_until_parked();
    save(&mut cx, expanded, &out, "expanded-card");

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
