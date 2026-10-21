//! Headless UI tests: the real board view in an in-process test window. Nothing here opens a desktop window, sends OS
//! input or changes focus outside the test.

use dashboard_core::{Board, EmptyState};
use gpui_kit::component::{ActiveTheme as _, Root};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, px, size};

use crate::board_view::BoardView;
use crate::theme;

fn open_board(cx: &mut TestAppContext, board: Board) -> (AnyWindowHandle, Entity<BoardView>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        theme::init(cx);
    });
    let mut view = None;
    let handle = cx.open_window(size(px(1100.), px(720.)), |window, cx| {
        let board_view = cx.new(|_| BoardView::new(board));
        view = Some(board_view.clone());
        Root::new(board_view, window, cx)
    });
    let handle: AnyWindowHandle = handle.into();
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
    (handle, view.unwrap())
}

#[gpui_kit::test]
fn empty_board_renders_its_empty_state(cx: &mut TestAppContext) {
    let (_, view) = open_board(cx, Board::default());
    let empty = cx.update(|cx| view.read(cx).board().empty_state());
    assert_eq!(empty, Some(EmptyState::NO_REPOSITORIES));
}

#[gpui_kit::test]
fn brand_theme_is_gold_on_black(cx: &mut TestAppContext) {
    open_board(cx, Board::default());
    let (primary, background) = cx.update(|cx| (cx.theme().primary, cx.theme().background));
    // #D4AF37 and #0A0A0A from themes/dashboard.json.
    assert_eq!(rgb_of(primary), (0xD4, 0xAF, 0x37));
    assert_eq!(rgb_of(background), (0x0A, 0x0A, 0x0A));
}

fn rgb_of(color: gpui_kit::Hsla) -> (u8, u8, u8) {
    let rgba = color.to_rgb();
    let channel = |value: f32| (value * 255.).round() as u8;
    (channel(rgba.r), channel(rgba.g), channel(rgba.b))
}
