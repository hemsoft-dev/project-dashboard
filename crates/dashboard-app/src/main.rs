// A release build is a windowed app: no console window behind it on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod board_view;
mod theme;
// Only Linux can render GPUI headlessly from a test.
#[cfg(all(test, target_os = "linux"))]
mod captures;
#[cfg(test)]
mod ui_tests;

use dashboard_core::{APP_NAME, Board};
use gpui_kit::component::TitleBar;
use gpui_kit::{AppContext as _, Bounds, WindowBounds, WindowOptions, px, size};

use crate::board_view::BoardView;

/// The Linux desktop identity the window manager groups the window by.
const APP_ID: &str = "HemSoft.ProjectDashboard";

fn main() {
    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(|cx| {
        gpui_kit::init(cx);
        theme::init(cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1100.), px(720.)),
                cx,
            ))),
            window_min_size: Some(size(px(480.), px(320.))),
            app_id: (!cfg!(windows)).then(|| APP_ID.to_owned()),
            ..TitleBar::window_options()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            window.set_window_title(APP_NAME);
            cx.new(|_| BoardView::new(Board::default()))
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
