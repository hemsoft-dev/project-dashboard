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
        body = body.child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(data.fetched_at.map_or_else(
                    || "Waiting for fleet data from pmd…".into(),
                    |at| {
                        format!(
                            "Read from pmd {} · observations older than 10 minutes are stale",
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
            "Sessions / cap",
            "Free slots",
            "Last checked",
            "Running work",
        ];
        let widths = Column::FLEET.map(|c| self.columns.get(c));
        let total: f32 = widths.iter().sum();
        let cell = |text: String, index: usize| {
            div()
                .id(("fleet-cell", index))
                .w(rems(widths[index]))
                .flex_none()
                .px_2()
                .child(text)
        };
        let mut table = v_flex()
            .w(rems(total))
            .flex_none()
            .gap_2()
            .child(
                h_flex()
                    .font_semibold()
                    .children(headers.into_iter().enumerate().map(|(i, h)| {
                        cell(h.into(), i)
                            .relative()
                            .child(self.resize_handle(Column::FLEET[i], cx))
                    })),
            );
        for machine in &data.machines {
            let obs = data.observations.iter().find(|o| o.name == machine.name);
            let load = data.loads.iter().find(|l| l.name == machine.name);
            let work = data
                .assignments
                .iter()
                .filter(|a| a.machine == machine.name && a.state == "running" && !a.stale)
                .map(|a| {
                    a.work_item.as_ref().map_or_else(
                        || a.thread_id.clone(),
                        |w| match w {
                            dashboard_core::backlog::WorkRef::Id(id)
                            | dashboard_core::backlog::WorkRef::Item { id } => id.clone(),
                        },
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let values = [
                format!("{} · {}", machine.name, machine.os),
                obs.map_or("Unknown", |o| o.connectivity(now)).into(),
                obs.map_or("Unknown", |o| o.server_status(now)).into(),
                format!(
                    "{} / {}",
                    obs.and_then(|o| o.server_version.as_deref()).unwrap_or("Unknown"),
                    obs.and_then(|o| o.latest_version.as_deref()).unwrap_or("Unknown")
                ),
                obs.and_then(|o| o.client_version.clone())
                    .unwrap_or_else(|| "Unknown".into()),
                format!(
                    "{} / {}",
                    if load_stale {
                        "Stale".into()
                    } else {
                        load.and_then(|l| l.used).map_or("Unknown".into(), |n| n.to_string())
                    },
                    machine.max_sessions
                ),
                if load_stale {
                    "Stale".into()
                } else if machine.max_sessions == 0 {
                    "N/A · disabled".into()
                } else {
                    load.and_then(|l| l.free).map_or("Unknown".into(), |n| n.to_string())
                },
                obs.and_then(|o| o.checked_at).map_or("Never".into(), |at| {
                    format!("{} · {}", clock_time(at, now), age_words(at, now))
                }),
                if load_stale || load.is_none_or(|l| l.load_unknown.is_some() || l.used.is_none()) {
                    load.and_then(|l| l.load_unknown.clone())
                        .unwrap_or_else(|| "Unknown · source stale".into())
                } else if work.is_empty() {
                    "None reported".into()
                } else {
                    work
                },
            ];
            table = table.child(
                v_flex()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(h_flex().children(values.into_iter().enumerate().map(|(i, v)| cell(v, i))))
                    .children(
                        obs.and_then(|o| o.error.clone())
                            .into_iter()
                            .map(|e| div().px_2().text_color(cx.theme().warning).child(e)),
                    ),
            );
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
                .flex_1()
                .min_h_0()
                .overflow_scroll()
                .child(table),
        )
    }
}
