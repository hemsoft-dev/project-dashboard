use crate::board_view::BoardView;
use dashboard_core::web_url::WebUrl;
use dashboard_core::{BacklogColumn, Direction};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, StyledExt as _, h_flex, tag::Tag, tooltip::Tooltip, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, Div, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, TestSupportExt as _, div, rems,
};

/// A Backlog column's size (#118). The header and every row size their cells the same way, so values line up under
/// their titles. Item takes the room left; the others shrink together in a narrow window. Item clips what doesn't fit,
/// so in a very narrow window its author-model tags are cut at the Reason column instead of drawn under it; the row's
/// tooltip and accessible label still carry them.
fn cell(column: BacklogColumn) -> Div {
    let cell = div().min_w_0();
    match column {
        BacklogColumn::Project => cell.w(rems(9.)).flex_shrink_1(),
        BacklogColumn::Priority => cell.w(rems(4.5)).flex_none(),
        BacklogColumn::Item => cell
            .flex_grow_1()
            .flex_shrink_1()
            .flex_basis(rems(12.))
            .overflow_hidden(),
        BacklogColumn::Reason => cell.w(rems(10.)).flex_shrink_1(),
        BacklogColumn::Holder => cell.w(rems(12.)).flex_shrink_1(),
    }
}

/// The position of `column` in the header, for its arrow's element id.
fn index(column: BacklogColumn) -> usize {
    BacklogColumn::ALL.iter().position(|&c| c == column).unwrap_or_default()
}

/// The sort arrow's element id, which names its direction, so tests can tell the arrows apart without reading text.
pub(crate) fn arrow_id(direction: Direction) -> &'static str {
    match direction {
        Direction::Ascending => "backlog-sort-ascending",
        Direction::Descending => "backlog-sort-descending",
    }
}

/// A Backlog header's element id.
pub(crate) fn sort_id(column: BacklogColumn) -> &'static str {
    match column {
        BacklogColumn::Project => "backlog-sort-project",
        BacklogColumn::Priority => "backlog-sort-priority",
        BacklogColumn::Item => "backlog-sort-item",
        BacklogColumn::Reason => "backlog-sort-reason",
        BacklogColumn::Holder => "backlog-sort-holder",
    }
}

impl BoardView {
    pub(crate) fn render_backlog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let cards = self.board.cards();
        let mut rows = dashboard_core::backlog::rows(cards.iter().filter_map(|c| c.document.as_ref()));
        self.backlog_sort.apply(&mut rows);
        let mut body = v_flex()
            .id("backlog")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_4()
            .pb_4()
            .when(rows.is_empty(), |body| body.pt_4())
            .gap_0();
        for card in cards {
            if let Some(error) = &card.error {
                body = body.child(div().text_color(cx.theme().muted_foreground).child(error.to_string()));
            }
            if let Some(h) = card.document.as_ref().and_then(|d| d.hosted.as_ref()) {
                if let Some(e) = &h.work_error {
                    body = body.child(div().child(format!("Backlog unavailable · {e} · showing last good work")));
                }
                if let Some(error) = &h.backlog.error {
                    body = body.child(div().child(format!("Backlog assignments/dispatch unavailable · {error}")));
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
        let header = (!rows.is_empty()).then(|| self.render_backlog_header(cx));
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
            let url = WebUrl::parse(row.url());
            let work = row.work_label();
            let title = format!("{} · {} · {}", row.project, row.priority_label(), work);
            let holder = row.workers.join(" / ");
            let detail = if row.reason.is_empty() {
                title
            } else {
                format!("{title} · {}", row.reason)
            };
            let full_label = std::iter::once(detail.as_str())
                .chain(std::iter::once(holder.as_str()))
                .chain(row.author_models.iter().map(String::as_str))
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            let tooltip = full_label.clone();
            let muted = cx.theme().muted_foreground;
            let line = h_flex()
                .id(("backlog-row", ix))
                .test_support()
                .aria_label(full_label)
                .h_8()
                .flex_none()
                .min_w_0()
                .px_2()
                .gap_2()
                .text_sm()
                .border_b_1()
                .border_color(cx.theme().border)
                .when_some(url, |line, url| {
                    line.cursor_pointer()
                        .hover(|s| s.bg(cx.theme().muted))
                        .on_click(move |_, _, cx| cx.open_url(url.as_str()))
                })
                .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                .child(
                    cell(BacklogColumn::Project)
                        .id(("backlog-project", ix))
                        .test_support()
                        .truncate()
                        .child(row.project.clone()),
                )
                .child(
                    cell(BacklogColumn::Priority)
                        .id(("backlog-priority", ix))
                        .test_support()
                        .child(row.priority_label()),
                )
                .child(
                    cell(BacklogColumn::Item).child(
                        h_flex()
                            .min_w_0()
                            .gap_2()
                            .child(
                                div()
                                    .id(("backlog-work", ix))
                                    .test_support()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(work),
                            )
                            .children(
                                row.author_models
                                    .into_iter()
                                    .map(|model| div().flex_none().child(Tag::new().child(model).small().outline())),
                            ),
                    ),
                )
                .child(
                    cell(BacklogColumn::Reason)
                        .id(("backlog-reason", ix))
                        .test_support()
                        .truncate()
                        .text_color(muted)
                        .child(row.reason),
                )
                .child(cell(BacklogColumn::Holder).when(!holder.is_empty(), |cell| {
                    cell.child(
                        div()
                            .id(("backlog-holder", ix))
                            .test_support()
                            .min_w_0()
                            .truncate()
                            .text_color(muted)
                            .child(holder),
                    )
                }));
            body = body.child(line);
        }
        v_flex().flex_1().min_h_0().children(header).child(body)
    }

    /// The Backlog's column headers (#118), aligned with the row cells. Each click sorts the rows within their stage
    /// group: ascending, descending, then pmd's dispatch order.
    fn render_backlog_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, foreground) = (theme.muted_foreground, theme.foreground);
        let headers = BacklogColumn::ALL.into_iter().map(|column| {
            let direction = self.backlog_sort.direction_of(column);
            cell(column).child(
                h_flex()
                    .id(sort_id(column))
                    .test_support()
                    .aria_label(self.backlog_sort.label(column))
                    .min_w_0()
                    .gap_1()
                    .cursor_pointer()
                    .text_color(muted)
                    .when(direction.is_some(), |this| this.text_color(foreground))
                    .hover(move |style| style.text_color(foreground))
                    .on_click(cx.listener(move |view, _, _, cx| view.cycle_backlog_sort(column, cx)))
                    .child(div().min_w_0().truncate().child(column.title()))
                    .children(direction.map(|direction| {
                        div()
                            .id((arrow_id(direction), index(column)))
                            .test_support()
                            .flex_none()
                            .child(direction.arrow())
                    })),
            )
        });
        div()
            .px_4()
            .pt_4()
            .child(
                h_flex()
                    .id("backlog-header")
                    .test_support()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .text_xs()
                    .font_semibold()
                    .border_b_1()
                    .border_color(theme.border)
                    .children(headers),
            )
            .into_any_element()
    }
}
