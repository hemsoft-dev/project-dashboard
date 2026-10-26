// A release build is a windowed app: no console window behind it on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod board_view;
// Only Linux can render GPUI headlessly from a test.
#[cfg(all(test, target_os = "linux"))]
mod captures;
mod demo;
mod mark;
mod theme;
#[cfg(test)]
mod ui_tests;
mod zoom;

use std::sync::Arc;

use dashboard_core::APP_NAME;
use dashboard_core::cache::StatusCache;
use dashboard_core::config::{self, Config};
use dashboard_core::source::StatusSource;
use dashboard_core::t3;
use dashboard_core::ui_state::UiState;
use gpui_kit::component::TitleBar;
use gpui_kit::{AppContext as _, Bounds, WindowBounds, WindowOptions, px, size};

use crate::board_view::{BoardView, Options};

/// The Linux desktop identity the window manager groups the window by.
const APP_ID: &str = "HemSoft.ProjectDashboard";

fn is_demo() -> bool {
    std::env::args().any(|arg| arg == "--demo")
}

fn main() {
    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(|cx| {
        gpui_kit::init(cx);
        theme::init(cx);
        board_view::bind_keys(cx);
        // The demo remembers its zoom too, so a restart keeps it.
        zoom::init(cx, Some(config::default_dir()));
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1100.), px(760.)),
                cx,
            ))),
            window_min_size: Some(size(px(480.), px(320.))),
            app_id: (!cfg!(windows)).then(|| APP_ID.to_owned()),
            ..TitleBar::window_options()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            window.set_window_title(APP_NAME);
            cx.new(|cx| {
                let mut view = if is_demo() {
                    demo_view(window, cx)
                } else {
                    live_view(window, cx)
                };
                view.refresh_all(cx);
                view
            })
        })
        .expect("failed to open the dashboard window");
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}

/// The configured checkouts, shown from the cache first and then refreshed from pmd.
fn live_view(window: &mut gpui_kit::Window, cx: &mut gpui_kit::Context<BoardView>) -> BoardView {
    let dir = config::default_dir();
    let (config, config_problem) = match Config::load_or_create(&dir) {
        Ok(config) => (config, None),
        Err(err) => (Config::default(), Some(board_view::config_problem(&err))),
    };
    let source: Arc<dyn StatusSource> = Arc::new(dashboard_core::priority::Pmd::from_config(&config, &dir));
    BoardView::new(
        &config.repositories,
        StatusCache::load(&dir),
        source,
        Options {
            cache_dir: Some(dir.clone()),
            parallelism: config.parallelism,
            auto_refresh: config.refresh_interval(),
            config_problem,
            config_path: Some(dir.join(config::FILE_NAME)),
            config_dir: Some(dir.clone()),
            now: None,
            columns: UiState::load(&dir).columns,
            ui_state_dir: Some(dir.clone()),
            priorities: Arc::new(dashboard_core::priority::Pmd::from_config(&config, &dir)),
            pmd: Some(dashboard_core::priority::Pmd::from_config(&config, &dir)),
            threads: Arc::new(t3::LocalT3 {
                home: config.t3_home.clone(),
            }),
        },
        window,
        cx,
    )
}

/// `--demo`: the fixtures, with no config, cache or gh involved.
fn demo_view(window: &mut gpui_kit::Window, cx: &mut gpui_kit::Context<BoardView>) -> BoardView {
    BoardView::new(
        &demo::checkouts(),
        StatusCache::default(),
        Arc::new(demo::DemoSource),
        Options {
            cache_dir: None,
            parallelism: 4,
            auto_refresh: None,
            config_problem: None,
            config_path: None,
            config_dir: None,
            // The fixtures' clock, so the demo's ages read the same whenever it runs.
            now: Some(demo::fixture_now()),
            // Like the zoom, the demo remembers its column widths.
            columns: UiState::load(&config::default_dir()).columns,
            ui_state_dir: Some(config::default_dir()),
            threads: Arc::new(demo::DemoThreads),
            priorities: Arc::new(demo::DemoPriorities),
            pmd: None,
        },
        window,
        cx,
    )
}
