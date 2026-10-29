use crate::board_view::BoardView;
use gpui_kit::component::{ActiveTheme as _, StyledExt as _, v_flex};
use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _, Styled as _,
    TestSupportExt as _, div,
};
impl BoardView {
    pub(crate) fn render_backlog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let cards = self.board.cards();
        let rows = dashboard_core::backlog::rows(cards.iter().filter_map(|c| c.document.as_ref()));
        let mut body = v_flex()
            .id("backlog")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_4()
            .gap_2();
        for card in cards {
            if let Some(error) = &card.error {
                body = body.child(div().text_color(cx.theme().muted_foreground).child(error.to_string()));
            }
            if let Some(h) = card.document.as_ref().and_then(|d| d.hosted.as_ref()) {
                if let Some(e) = &h.work_error {
                    body = body.child(div().child(format!("Backlog unavailable · {e} · showing last good work")));
                }
                if h.backlog.plan.items_truncated || h.backlog.plan.skipped_truncated {
                    body = body.child(
                        div().child("Dispatch plan incomplete; remaining Ready items use pmd priority and rank."),
                    );
                }
            }
        }
        if rows.is_empty() {
            body = body.child(
                div()
                    .id("backlog-empty")
                    .test_support()
                    .p_4()
                    .child("No backlog items available. Refresh to read work from pmd."),
            );
        }
        let mut group = None;
        for (ix, row) in rows.into_iter().enumerate() {
            if group != Some(row.group) {
                group = Some(row.group);
                body = body.child(
                    div()
                        .mt_4()
                        .font_semibold()
                        .text_color(cx.theme().primary)
                        .child(row.group.title()),
                );
            }
            let url = row.url().to_owned();
            let title = row
                .items
                .iter()
                .map(|i| format!("{} #{} · {} · {}", i.kind, i.number, i.title, i.detail.gh_state))
                .collect::<Vec<_>>()
                .join("  /  ");
            let mut line = v_flex()
                .id(("backlog-row", ix))
                .test_support()
                .p_3()
                .gap_1()
                .border_b_1()
                .border_color(cx.theme().border)
                .cursor_pointer()
                .hover(|s| s.bg(cx.theme().muted))
                .on_click(move |_, _, cx| cx.open_url(&url))
                .child(
                    div()
                        .font_semibold()
                        .child(format!("{} · {}", row.project, row.priority_label())),
                )
                .child(div().child(title));
            for text in row
                .reviews
                .into_iter()
                .chain(row.workers)
                .chain(std::iter::once(row.reason))
                .filter(|s| !s.is_empty())
            {
                line = line.child(div().text_sm().text_color(cx.theme().muted_foreground).child(text));
            }
            body = body.child(line);
        }
        body
    }
}
