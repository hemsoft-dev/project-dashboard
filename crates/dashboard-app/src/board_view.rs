//! The dashboard window's content: a title bar and the board. With no repositories configured, the board is an
//! empty-state message.

use dashboard_core::{APP_NAME, Board};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, StyledExt as _, TitleBar, h_flex, v_flex};
use gpui_kit::{Context, IntoElement, ParentElement as _, Render, Styled as _, Window, div};

pub struct BoardView {
    board: Board,
}

impl BoardView {
    pub fn new(board: Board) -> Self {
        Self { board }
    }

    #[cfg(test)]
    pub fn board(&self) -> &Board {
        &self.board
    }

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        TitleBar::new().child(
            h_flex()
                .gap_2()
                .items_center()
                .child(Icon::new(IconName::LayoutDashboard).text_color(cx.theme().primary))
                .child(div().font_semibold().child(APP_NAME)),
        )
    }
}

impl Render for BoardView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.board.empty_state() {
            Some(empty) => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    Icon::new(IconName::FolderOpen)
                        .size_8()
                        .text_color(cx.theme().muted_foreground),
                )
                .child(div().text_lg().font_semibold().child(empty.title))
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(empty.hint),
                ),
            None => v_flex().flex_1(),
        };
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_title_bar(cx))
            .child(body)
    }
}
