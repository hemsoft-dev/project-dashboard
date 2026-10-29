use crate::board_view::BoardView;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, StyledExt as _, h_flex, tag::Tag, tooltip::Tooltip, v_flex};
use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _, Styled as _,
    TestSupportExt as _, div, rems,
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
            .gap_0();
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
                        .mt_3()
                        .mb_1()
                        .font_semibold()
                        .text_color(cx.theme().primary)
                        .child(row.group.title()),
                );
            }
            let url = row.url().to_owned();
            let title = format!("{} · {} · {}", row.project, row.priority_label(), row.work_label());
            let holder = if row.workers.is_empty() {
                "unassigned".to_owned()
            } else {
                row.workers.join(" / ")
            };
            let detail = if row.reason.is_empty() {
                title
            } else {
                format!("{title} · {}", row.reason)
            };
            let full_label = format!("{detail} · holder ({holder}) · {}", row.author_models.join(" · "));
            let tooltip = full_label.clone();
            let line = h_flex()
                .id(("backlog-row", ix))
                .test_support()
                .aria_label(full_label)
                .h_8()
                .flex_none()
                .min_w_0()
                .px_2()
                .gap_2()
                .border_b_1()
                .border_color(cx.theme().border)
                .cursor_pointer()
                .hover(|s| s.bg(cx.theme().muted))
                .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                .on_click(move |_, _, cx| cx.open_url(&url))
                .child(
                    div()
                        .id(("backlog-work", ix))
                        .test_support()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .truncate()
                        .child(detail),
                )
                .child(
                    div()
                        .id(("backlog-holder", ix))
                        .test_support()
                        .max_w(rems(20.))
                        .min_w_0()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .truncate()
                        .child(format!("· holder ({holder})")),
                )
                .children(
                    row.author_models
                        .into_iter()
                        .map(|model| div().flex_none().child(Tag::new().child(model).small().outline())),
                );
            body = body.child(line);
        }
        body
    }
}
