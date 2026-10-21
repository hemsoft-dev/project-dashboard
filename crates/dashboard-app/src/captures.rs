//! Screenshots for pull requests, rendered offscreen by GPUI's Linux headless renderer (wgpu), so no window ever appears
//! on a desktop. Windows has no headless renderer in GPUI.
//!
//! `CAPTURE_DIR=<folder> cargo test -p dashboard-app captures -- --ignored` writes one PNG per view.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dashboard_core::Board;
use gpui_kit::component::Root;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, HeadlessAppContext, px, size};

use crate::board_view::BoardView;
use crate::theme;

fn save(cx: &mut HeadlessAppContext, handle: AnyWindowHandle, dir: &Path, name: &str) {
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
    let image = cx
        .capture_screenshot(handle)
        .expect("the headless renderer draws the window");
    image.save(dir.join(format!("{name}.png"))).unwrap();
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
    let handle = cx
        .open_window(size(px(1100.), px(720.)), |window, cx| {
            let view = cx.new(|_| BoardView::new(Board::default()));
            cx.new(|cx| Root::new(view, window, cx))
        })
        .unwrap();
    cx.run_until_parked();
    save(&mut cx, handle.into(), &out, "empty-board");
}
