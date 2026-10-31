use crate::board_view::BoardView;
use dashboard_core::activity::{age_words, clock_time};
use dashboard_core::columns::Column;
use gpui_kit::component::{ActiveTheme as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _, Styled as _,
    TestSupportExt as _, div, rems,
};

impl BoardView {
    pub(crate) fn render_fleet(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let now = chrono::Utc::now();
        let data = &self.fleet_status;
        let load_stale = data
            .fetched_at
            .is_none_or(|at| now.signed_duration_since(at).num_seconds() > 600);
        let mut body = v_flex()
            .id("fleet-status")
            .test_support()
            .flex_1()
            .min_h_0()
            .gap_2()
            .p_3();
        if let Some(error) = &self.fleet_error {
            body = body.child(div().text_color(cx.theme().warning).child(format!(
                "Fleet refresh failed. {error} Last good observations retained. Use F5 to retry."
            )));
        }
        if let Some(error) = &self.fleet_activity_error {
            body = body.child(div().text_color(cx.theme().warning).child(format!(
                "Runtime activity unavailable. {error} Last observations retained with their original age."
            )));
        }
        body = body.child(div().text_sm().text_color(cx.theme().muted_foreground)
            .child("Active work includes runtime monitoring and visible delegates. Chief allocation and free slots are dispatch headroom."));
        body = body.child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(data.fetched_at.map_or_else(
                || "Waiting for fleet data from pmd…".into(),
                |at| {
                    format!(
                        "Read from pmd {} · version probes expire after 10 minutes; runtime activity after 90 seconds",
                        age_words(at, now)
                    )
                },
            )),
        );
        let headers = [
            "Machine",
            "Connectivity",
            "T3 server",
            "Running / latest",
            "T3 client",
            "Active\nChief cap",
            "Chief free",
            "Last checked",
            "Running work",
        ];
        let widths = Column::FLEET.map(|c| self.columns.get(c));
        let total: f32 = widths.iter().sum();
        let cell = |text: String, index: usize| {
            div().w(rems(widths[index])).flex_none().px_2().child(
                v_flex().w_full().children(
                    text.lines()
                        .map(|line| div().w_full().truncate().child(line.to_owned())),
                ),
            )
        };
        // The shared handle extends half a rem beyond its cell, including the final column.
        let mut table = v_flex()
            .w(rems(total + 0.5))
            .flex_none()
            .gap_2()
            .child(
                h_flex()
                    .font_semibold()
                    .children(headers.into_iter().enumerate().map(|(i, h)| {
                        cell(h.into(), i)
                            .id(("fleet-header", i))
                            .test_support()
                            .relative()
                            .child(self.resize_handle(Column::FLEET[i], cx))
                    })),
            );
        for (row, machine) in data.machines.iter().enumerate() {
            let obs = data.observations.iter().find(|o| o.name == machine.name);
            let load = data.loads.iter().find(|l| l.name == machine.name);
            let activity = self.fleet_activity.machines.iter().find(|m| m.name == machine.name);
            let activity_failed = self.fleet_activity_error.is_some();
            let active = activity.map_or_else(
                || "Unavailable".into(),
                |m| {
                    if activity_failed {
                        "Unavailable".into()
                    } else {
                        m.count_label(now)
                    }
                },
            );
            let work = activity.map_or_else(
                || "Runtime activity unavailable".into(),
                |m| m.work_label(now, activity_failed),
            );
            let values = [
                format!("{} · {}", machine.name, machine.os),
                obs.map_or("Unknown", |o| o.connectivity(now)).into(),
                obs.map_or("Unknown", |o| o.server_status(now)).into(),
                format!(
                    "{} / {}",
                    obs.and_then(|o| o.server_version.as_deref()).unwrap_or("Unknown"),
                    obs.and_then(|o| o.latest_version.as_deref()).unwrap_or("Unknown")
                ),
                if obs.and_then(|o| o.client_status.as_deref()) == Some("not_applicable") {
                    "N/A".into()
                } else {
                    obs.and_then(|o| o.client_version.clone())
                        .unwrap_or_else(|| "Unknown".into())
                },
                format!(
                    "{}\nChief {} / {}",
                    active,
                    if load_stale {
                        "Stale".into()
                    } else {
                        load.and_then(|l| l.chief_started)
                            .map_or("Unknown".into(), |n| n.to_string())
                    },
                    machine.max_sessions
                ),
                if load_stale {
                    "Stale".into()
                } else if machine.max_sessions == 0 {
                    "N/A · disabled".into()
                } else {
                    load.and_then(|l| l.free)
                        .map_or("Unknown".into(), |n| format!("{n} dispatch"))
                },
                obs.and_then(|o| o.checked_at).map_or("Never".into(), |at| {
                    format!("{} · {}", clock_time(at, now), age_words(at, now))
                }),
                work,
            ];
            table = table.child(
                v_flex()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(h_flex().children(values.into_iter().enumerate().map(|(i, v)| {
                        let full = v.clone();
                        cell(v, i)
                            .id(("fleet-body", row * 9 + i))
                            .test_support()
                            .aria_label(full.clone())
                            .tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(full.clone()).build(window, cx)
                            })
                    })))
                    .children(
                        obs.and_then(|o| o.error.clone())
                            .into_iter()
                            .map(|e| div().px_2().text_color(cx.theme().warning).child(e)),
                    ),
            );
            if let Some(activity) = activity {
                if let Some(at) = activity.observed_at {
                    table = table.child(
                        div()
                            .px_2()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{} · runtime observed {}", machine.name, age_words(at, now))),
                    );
                }
                if let Some(reason) = &activity.reason {
                    table = table.child(
                        div()
                            .px_2()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{} · {reason}", machine.name)),
                    );
                }
                if activity.chief_active {
                    table = table.child(
                        div()
                            .px_2()
                            .text_sm()
                            .child("Chief is active (excluded from the session count)."),
                    );
                }
            }
        }
        if data.truncated {
            body = body.child(
                div()
                    .text_color(cx.theme().warning)
                    .child("Running work list is truncated by pmd."),
            );
        }
        if data.machines.is_empty() && data.fetched_at.is_some() {
            table = table.child(div().child("No fleet machines registered in pmd."));
        }
        body.child(
            div()
                .id("fleet-scroll")
                .test_support()
                .flex_1()
                .min_h_0()
                .overflow_scroll()
                .child(table),
        )
    }
}
