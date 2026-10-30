//! Project-only dispatch explanations, waiting items and source observations supplied by pmd.
use crate::{
    activity,
    pmd::{Hosted, WorkItem},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DispatchReason {
    pub project: String,
    pub reason: String,
    pub count: u64,
    pub owners: Vec<Owner>,
    pub hold_sources: Vec<String>,
    pub subscriptions: Vec<String>,
    pub stale: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Owner {
    pub machine: String,
    pub agent: Option<String>,
    pub owner: String,
    pub state: String,
    pub ownership: String,
}
impl DispatchReason {
    pub fn label(&self) -> &str {
        match self.reason.as_str() {
            "empty_backlog" => "Empty backlog",
            "all_owned" => "All owned",
            "held" => "Held",
            "waiting_on_franz" => "Waiting on Franz",
            "capacity_gated" => "Capacity gated",
            "dispatchable" => "Dispatchable",
            _ => "—",
        }
    }
    pub fn tooltip(&self) -> String {
        let mut lines = vec![format!("{} · {} work items", self.label(), self.count)];
        for owner in &self.owners {
            lines.push(format!(
                "Owner: {} · {} · {} · {}",
                if owner.owner.is_empty() {
                    owner.agent.as_deref().unwrap_or("unknown")
                } else {
                    &owner.owner
                },
                owner.machine,
                owner.state,
                owner.ownership
            ));
        }
        if !self.hold_sources.is_empty() {
            lines.push(format!("Holds: {}", self.hold_sources.join(", ")));
        }
        if !self.subscriptions.is_empty() {
            lines.push(format!("Capacity: {}", self.subscriptions.join(", ")));
        }
        if self.stale {
            lines.push("Dispatch explanation uses incomplete or stale source data".into());
        }
        lines.join("\n")
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Source {
    pub last_success_at: Option<String>,
    pub last_attempt_at: Option<String>,
    pub state: String,
    pub reason: String,
    pub stale_after_seconds: i64,
}
impl Source {
    pub fn state_at(&self, now: DateTime<Utc>) -> &str {
        if self.state == "fresh" {
            if let Some(at) = self.last_success_at.as_deref().and_then(activity::parse_time)
                && self.stale_after_seconds > 0
                && (now - at).num_seconds() > self.stale_after_seconds
            {
                return "stale";
            }
            if self.last_success_at.as_deref().and_then(activity::parse_time).is_some() {
                "fresh"
            } else {
                "unknown"
            }
        } else if self.state.is_empty() {
            "unknown"
        } else {
            &self.state
        }
    }
    pub fn tooltip(&self, name: &str, now: DateTime<Utc>) -> String {
        let local = |at: Option<&str>| {
            at.and_then(activity::parse_time)
                .map(|t| activity::time_in(t, &chrono::Local))
                .unwrap_or_else(|| "not observed".into())
        };
        format!(
            "{name}: {}\nLast success: {}\nLast attempt: {}\n{}",
            self.state_at(now),
            local(self.last_success_at.as_deref()),
            local(self.last_attempt_at.as_deref()),
            self.reason.replace('_', " ")
        )
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Portfolio {
    pub freshness: Option<Freshness>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Freshness {
    pub github: Source,
    pub capacity: Source,
    pub gh_x: BTreeMap<String, Source>,
    pub t3: BTreeMap<String, Source>,
}
impl Freshness {
    pub fn for_repository(self, repository: &str) -> BTreeMap<String, Source> {
        let gh_x = self
            .gh_x
            .into_iter()
            .find(|(repo, _)| repo.eq_ignore_ascii_case(repository))
            .map(|(_, s)| s)
            .unwrap_or_default();
        let mut sources = BTreeMap::from([
            ("GitHub".into(), self.github),
            ("Capacity".into(), self.capacity),
            (format!("gh-x/{repository}"), gh_x),
        ]);
        if self.t3.is_empty() {
            sources.insert("T3".into(), Source::default());
        }
        for (scope, source) in self.t3 {
            sources.insert(format!("T3/{scope}"), source);
        }
        sources
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Readiness {
    pub dispatch: Option<DispatchReason>,
    pub dispatch_unavailable: bool,
    pub sources: BTreeMap<String, Source>,
    pub sources_unavailable: bool,
}
impl Readiness {
    pub fn stale(&self, now: DateTime<Utc>) -> bool {
        self.sources_unavailable
            || self.sources.values().any(|s| s.state_at(now) != "fresh")
            || self.dispatch_unavailable
            || self.dispatch.as_ref().is_some_and(|d| d.stale)
    }
}
pub fn oldest_waiting(hosted: &Hosted) -> Option<(&WorkItem, DateTime<Utc>)> {
    let work: Vec<_> = if hosted.work.is_empty() {
        if hosted.board.as_ref().is_some_and(|b| {
            b.columns
                .iter()
                .any(|c| c.truncated && matches!(c.status.as_str(), "blocked" | "waiting_on_franz"))
        }) {
            return None;
        }
        hosted
            .board
            .iter()
            .flat_map(|b| &b.columns)
            .flat_map(|c| &c.items)
            .collect()
    } else {
        hosted.work.iter().collect()
    };
    work.into_iter()
        .filter(|w| w.detail.gh_state != "closed" && w.detail.gh_state != "merged")
        .filter(|w| matches!(w.status.as_str(), "blocked" | "waiting_on_franz"))
        .filter_map(|w| Some((w, w.detail.waiting_since.as_deref().and_then(activity::parse_time)?)))
        .min_by(|(a, at), (b, bt)| at.cmp(bt).then_with(|| a.id.cmp(&b.id)))
}
pub fn waiting_tooltip(item: &WorkItem, since: DateTime<Utc>) -> String {
    format!(
        "{} #{}: {}\nWaiting since {}\n{}",
        if item.kind == "pr" { "PR" } else { "Issue" },
        item.number,
        item.title,
        activity::time_in(since, &chrono::Local),
        item.detail
            .waiting_reason
            .as_deref()
            .or(item.detail.status_reason.as_deref())
            .unwrap_or("Reason unavailable")
    )
}
