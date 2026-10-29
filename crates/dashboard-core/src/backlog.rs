//! Live backlog projection. Relationships and dispatch decisions come only from pmd.
use crate::{
    pmd::{ReadApi, WorkItem},
    source::FetchError,
    status::StatusDocument,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Detail {
    pub gh_state: String,
    pub labels: Vec<String>,
    pub priority: Option<u8>,
    pub rank: i64,
    pub links: Vec<Link>,
    pub status_reason: Option<String>,
    pub assignment: Option<Assignment>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Link {
    pub kind: String,
    pub to: Option<String>,
    pub by: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Assignment {
    pub id: String,
    pub machine: String,
    pub thread_id: String,
    pub model: String,
    pub state: String,
    pub holds: Option<bool>,
    pub stale: bool,
    pub ownership: String,
    pub ended_at: Option<String>,
    pub work_item: Option<WorkRef>,
}
/// The work list uses an id; the assignment list uses an item reference.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WorkRef {
    Id(String),
    Item { id: String },
}
impl WorkRef {
    fn matches(&self, item: &str) -> bool {
        match self {
            Self::Id(id) | Self::Item { id } => id == item,
        }
    }
}
impl Assignment {
    /// Holding work is broader than doing work: pmd retains idle needs-check holders.
    fn live(&self) -> bool {
        matches!(self.state.as_str(), "running" | "queued" | "waiting_input")
            && !self.stale
            && self.ended_at.is_none()
            && self.ownership != "settled-complete"
            && self.holds != Some(false)
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Context {
    pub assignments: Vec<Assignment>,
    pub plan: Plan,
    /// Assignment/dispatch failures are independent of the Project board/work read.
    pub error: Option<String>,
    pub transport_error: Option<crate::source::TransportError>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Plan {
    pub recommendations: Vec<Decision>,
    pub skipped: Vec<Decision>,
    pub skipped_truncated: bool,
    pub items_truncated: bool,
    pub blocked: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Decision {
    pub work_item: Ref,
    pub reason: String,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ref {
    pub id: String,
}
pub(crate) fn read(api: &ReadApi, slug: &str) -> Result<Context, FetchError> {
    #[derive(Deserialize)]
    struct Page {
        assignments: Vec<Assignment>,
        truncated: bool,
    }
    let page: Page = api.get("v1/assignments", &[("project", slug), ("limit", "500")])?;
    if page.truncated {
        return Err(FetchError::Failed("pmd assignments truncated".into()));
    }
    let plan = api.get("v1/dispatch/plan", &[])?;
    Ok(Context {
        assignments: page.assignments,
        plan,
        ..Default::default()
    })
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    InProgress,
    UpNext,
    Waiting,
    Later,
}
impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Self::InProgress => "In progress",
            Self::UpNext => "Up next",
            Self::Waiting => "Waiting / Blocked",
            Self::Later => "P4 and later",
        }
    }
}
#[derive(Clone, Debug)]
pub struct Row {
    pub project: String,
    pub items: Vec<WorkItem>,
    pub group: Group,
    pub reason: String,
    pub workers: Vec<String>,
    pub reviews: Vec<String>,
    pub author_models: Vec<String>,
    priority: u8,
    rank: i64,
    dispatch: usize,
}
impl Row {
    pub fn work_label(&self) -> String {
        let issues = self
            .items
            .iter()
            .filter(|i| i.kind == "issue")
            .map(|i| format!("#{} {}", i.number, i.title))
            .collect::<Vec<_>>();
        let prs = self
            .items
            .iter()
            .filter(|i| i.kind == "pr")
            .map(|i| {
                let detail = self
                    .reviews
                    .iter()
                    .find(|r| r.starts_with(&format!("PR #{} (", i.number)))
                    .cloned()
                    .unwrap_or_else(|| format!("PR #{} ({}, review unavailable)", i.number, i.detail.gh_state));
                if issues.is_empty() {
                    format!("{} {}", detail, i.title)
                } else {
                    detail
                }
            })
            .collect::<Vec<_>>();
        [issues.join(" / "), prs.join(" / ")]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" → ")
    }

    pub fn priority_label(&self) -> String {
        if self.priority <= 5 {
            format!("P{}", self.priority)
        } else {
            "—".into()
        }
    }
    pub fn url(&self) -> &str {
        self.items
            .iter()
            .find(|i| i.kind == "pr")
            .unwrap_or(&self.items[0])
            .url
            .as_str()
    }
}
pub fn rows<'a>(documents: impl IntoIterator<Item = &'a StatusDocument>) -> Vec<Row> {
    let mut result = Vec::new();
    let mut seen_projects = HashSet::new();
    for doc in documents {
        let Some(hosted) = &doc.hosted else { continue };
        let Some(first) = hosted.work.first() else { continue };
        if !seen_projects.insert(first.project.clone()) {
            continue;
        }
        let mut unseen: HashSet<_> = hosted
            .work
            .iter()
            .filter(|i| i.detail.gh_state != "closed" && i.detail.gh_state != "merged")
            .map(|i| i.id.clone())
            .collect();
        for seed in &hosted.work {
            if !unseen.remove(&seed.id) {
                continue;
            }
            let mut items = vec![seed.clone()];
            loop {
                let related: Vec<_> = hosted
                    .work
                    .iter()
                    .filter(|i| unseen.contains(&i.id) && items.iter().any(|j| linked(i, j) || linked(j, i)))
                    .cloned()
                    .collect();
                if related.is_empty() {
                    break;
                }
                for i in related {
                    unseen.remove(&i.id);
                    items.push(i);
                }
            }
            items.sort_by_key(|i| (i.kind != "issue", i.number));
            let mut workers = Vec::new();
            let mut author_models = Vec::new();
            let mut waiting_reasons = Vec::new();
            let mut reviews = Vec::new();
            let mut active = false;
            let mut waiting = false;
            let mut dispatch = usize::MAX;
            for item in &items {
                let d = &item.detail;
                let item_waiting = matches!(
                    item.status.as_str(),
                    "waiting_on_franz" | "blocked" | "watching" | "parked"
                ) || d.labels.iter().any(|l| {
                    matches!(
                        l.as_str(),
                        "waiting-on-franz" | "waiting_on_franz" | "blocked" | "watching" | "parked"
                    )
                });
                waiting |= item_waiting;
                if item_waiting {
                    if let Some(reason) = &d.status_reason {
                        waiting_reasons.push(reason.clone());
                    }
                    // Only the waiting member contributes to a grouped row's waiting reasons.
                    if matches!(
                        item.status.as_str(),
                        "waiting_on_franz" | "blocked" | "watching" | "parked"
                    ) {
                        waiting_reasons.push(item.status.replace('_', " "));
                    }
                    waiting_reasons.extend(
                        d.labels
                            .iter()
                            .filter(|l| {
                                matches!(
                                    l.as_str(),
                                    "waiting-on-franz" | "waiting_on_franz" | "blocked" | "watching" | "parked"
                                )
                            })
                            .map(|l| l.replace('_', " ")),
                    );
                }
                // Prefer the richer current assignment list over the work item's summary.
                // Also consider another live holder when the newest summary is idle.
                let assignments: Vec<_> = hosted
                    .backlog
                    .assignments
                    .iter()
                    .filter(|a| {
                        hosted.backlog.error.is_none()
                            && (d.assignment.as_ref().is_some_and(|summary| summary.id == a.id)
                                || a.work_item.as_ref().is_some_and(|r| r.matches(&item.id)))
                    })
                    .collect();
                if assignments.is_empty() {
                    if let Some(a) = d.assignment.as_ref().filter(|a| a.live()) {
                        active = true;
                        workers.push(format!("{} · {}", a.machine, a.model));
                    }
                } else {
                    for a in assignments.into_iter().filter(|a| a.live()) {
                        active = true;
                        workers.push(format!("{} · {}", a.machine, a.model));
                    }
                }
                for label in &d.labels {
                    if label.starts_with("model:") {
                        author_models.push(label.clone());
                    }
                }
                if let Some(pr) = doc
                    .pull_requests
                    .open
                    .iter()
                    .find(|p| item.kind == "pr" && p.number == item.number)
                {
                    let review = [&pr.review, &pr.ai_review]
                        .into_iter()
                        .filter(|s| !s.is_empty() && s.as_str() != "—" && s.as_str() != "-")
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join(", ");
                    reviews.push(format!(
                        "PR #{} ({}, {})",
                        pr.number,
                        pr.state,
                        if review.is_empty() {
                            "review unavailable"
                        } else {
                            &review
                        }
                    ));
                }
                if let Some(n) = hosted
                    .backlog
                    .plan
                    .recommendations
                    .iter()
                    .position(|d| d.work_item.id == item.id)
                {
                    dispatch = dispatch.min(n);
                }
            }
            author_models.sort();
            author_models.dedup();
            workers.sort();
            workers.dedup();
            // Runtime/dispatch reasons may carry opaque thread ids. Keep the visible
            // explanation limited to the waiting work members, in human language.
            let mut reasons: Vec<_> = waiting_reasons
                .iter()
                .flat_map(|r| r.split('·'))
                .filter_map(waiting_reason)
                .collect();
            reasons.sort();
            reasons.dedup();
            let priority = items.iter().filter_map(|i| i.detail.priority).min().unwrap_or(9);
            let rank = items.iter().map(|i| i.detail.rank).min().unwrap_or(0);
            let group = if waiting {
                Group::Waiting
            } else if active {
                Group::InProgress
            } else if priority >= 4 {
                Group::Later
            } else if items
                .iter()
                .any(|i| matches!(i.status.as_str(), "ready" | "in_progress"))
            {
                Group::UpNext
            } else {
                Group::Waiting
            };
            result.push(Row {
                project: seed.project.clone(),
                items,
                group,
                reason: reasons.join(" · "),
                workers,
                reviews,
                author_models,
                priority,
                rank,
                dispatch,
            });
        }
    }
    result.sort_by(|a, b| {
        (a.group, a.priority, a.rank, a.dispatch, &a.project, &a.items[0].id).cmp(&(
            b.group,
            b.priority,
            b.rank,
            b.dispatch,
            &b.project,
            &b.items[0].id,
        ))
    });
    result
}
fn waiting_reason(reason: &str) -> Option<String> {
    let reason = reason.trim();
    let lower = reason.to_ascii_lowercase();
    if reason.is_empty() || lower.contains("thread:") || lower.contains("thread%3a") || lower.contains("delegated-task")
    {
        return None;
    }
    let words = lower
        .replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    Some(match words.as_str() {
        "waiting on franz" => "waiting on Franz".into(),
        "blocked" | "watching" | "parked" => words,
        _ => reason.to_owned(),
    })
}

fn linked(a: &WorkItem, b: &WorkItem) -> bool {
    a.project == b.project
        && a.detail
            .links
            .iter()
            .any(|l| l.kind == "closes" && (l.to.as_deref() == Some(&b.id) || l.by.as_deref() == Some(&b.id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn item(id: &str, kind: &str, status: &str, priority: u8, rank: i64) -> WorkItem {
        serde_json::from_value(json!({"id":id,"project":"example","kind":kind,"number":1,"title":id,"url":"https://github.com/o/r/issues/1","status":status,"ghState":"open","priority":priority,"rank":rank})).unwrap()
    }
    fn document(items: Vec<WorkItem>) -> StatusDocument {
        let mut doc = StatusDocument::parse(crate::fixtures::CLEAN).unwrap();
        doc.hosted = Some(crate::pmd::Hosted {
            service_url: "http://pmd".into(),
            generated_at: "2026-10-28T00:00:00Z".into(),
            checked_at: None,
            last_error: None,
            board: None,
            work: items,
            backlog: Context::default(),
            work_error: None,
            work_transport_error: None,
        });
        doc
    }
    #[test]
    fn idle_holders_do_not_make_in_progress_status_live() {
        let mut i = item("chief-9", "issue", "in_progress", 1, 0);
        i.detail.assignment = Some(Assignment {
            id: "old".into(),
            state: "idle".into(),
            ..Default::default()
        });
        let doc = document(vec![i]);
        assert_eq!(rows([&doc])[0].group, Group::UpNext);
    }
    #[test]
    fn closing_links_group_transitively_and_keep_unrelated_items() {
        let issue = item("issue", "issue", "ready", 3, 0);
        let mut pr = item("pr", "pr", "in_progress", 3, 0);
        pr.detail.assignment = Some(Assignment {
            state: "running".into(),
            ..Default::default()
        });
        pr.detail.links.push(Link {
            kind: "closes".into(),
            to: Some("issue".into()),
            by: None,
        });
        let doc = document(vec![issue, pr, item("other", "issue", "ready", 0, 0)]);
        let rows = rows([&doc]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].group, Group::InProgress);
        assert_eq!(rows[0].items.len(), 2);
        assert_eq!(rows[0].items[0].kind, "issue");
    }
    #[test]
    fn stages_then_priority_then_rank_define_predictable_order() {
        let mut active = item("active", "pr", "in_progress", 3, 0);
        active.detail.assignment = Some(Assignment {
            state: "running".into(),
            ..Default::default()
        });
        let doc = document(vec![
            item("later", "issue", "ready", 4, 0),
            item("wait", "issue", "blocked", 0, 0),
            item("p3", "issue", "ready", 3, 0),
            item("rank2", "issue", "ready", 0, 2),
            item("rank1", "issue", "ready", 0, 1),
            active,
        ]);
        assert_eq!(
            rows([&doc]).iter().map(|r| r.items[0].id.as_str()).collect::<Vec<_>>(),
            ["active", "rank1", "rank2", "p3", "wait", "later"]
        );
    }
    #[test]
    fn empty_and_offline_snapshot_are_distinct() {
        assert!(rows([&document(vec![])]).is_empty());
        let mut doc = document(vec![item("cached", "issue", "ready", 1, 0)]);
        doc.hosted.as_mut().unwrap().work_error = Some("offline".into());
        assert_eq!(rows([&doc])[0].items[0].id, "cached");
    }
    #[test]
    fn author_model_and_assignment_model_have_distinct_sources() {
        let mut i = item("issue", "issue", "in_progress", 1, 0);
        i.detail.labels.push("model:gpt-6.1-sol".into());
        i.detail.assignment = Some(Assignment {
            id: "a".into(),
            machine: "mini".into(),
            thread_id: "thread".into(),
            ..Default::default()
        });
        let mut doc = document(vec![i]);
        doc.hosted.as_mut().unwrap().backlog.assignments.push(Assignment {
            id: "a".into(),
            state: "running".into(),
            model: "claude".into(),
            ..Default::default()
        });
        let projected = rows([&doc]);
        let workers = &projected[0].workers;
        assert!(projected[0].author_models.contains(&"model:gpt-6.1-sol".into()));
        assert!(workers.iter().any(|w| w.contains("claude")));
    }
    #[test]
    fn only_live_holders_count_and_full_assignment_state_is_authoritative() {
        for state in [
            "running",
            "queued",
            "waiting_input",
            "idle",
            "done",
            "failed",
            "completed",
            "unknown",
            "",
        ] {
            let mut i = item("held", "issue", "in_progress", 1, 0);
            i.detail.assignment = Some(Assignment {
                id: "owner".into(),
                state: "running".into(),
                ..Default::default()
            });
            let mut doc = document(vec![i]);
            doc.hosted.as_mut().unwrap().backlog.assignments.push(Assignment {
                id: "owner".into(),
                state: state.into(),
                ..Default::default()
            });
            let group = if matches!(state, "running" | "queued" | "waiting_input") {
                Group::InProgress
            } else {
                Group::UpNext
            };
            assert_eq!(rows([&doc])[0].group, group, "state {state}");
        }
    }
    #[test]
    fn stale_ended_settled_and_released_holders_are_not_live() {
        for patch in [
            json!({"stale":true}),
            json!({"endedAt":"2026-10-28T00:00:00Z"}),
            json!({"ownership":"settled-complete"}),
            json!({"holds":false}),
        ] {
            let mut value = json!({"id":"owner","state":"running","holds":true});
            value
                .as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            let mut i = item("held", "issue", "in_progress", 1, 0);
            i.detail.assignment = Some(serde_json::from_value(value).unwrap());
            assert_eq!(rows([&document(vec![i])])[0].group, Group::UpNext);
        }
    }
    #[test]
    fn idle_blocked_holders_wait_with_reason_and_unowned_prs_do_not_imply_activity() {
        let mut blocked = item("blocked", "issue", "in_progress", 1, 0);
        blocked.detail.labels.push("waiting-on-franz".into());
        blocked.detail.status_reason = Some("Needs credentials".into());
        blocked.detail.assignment = Some(Assignment {
            state: "idle".into(),
            ..Default::default()
        });
        let mut doc = document(vec![blocked, item("pr", "pr", "ready", 1, 0)]);
        doc.pull_requests
            .open
            .push(crate::fixtures::document(crate::fixtures::ATTENTION).pull_requests.open[0].clone());
        doc.pull_requests.open[0].number = 1;
        let projected = rows([&doc]);
        assert_eq!(projected[0].group, Group::UpNext);
        assert_eq!(projected[1].group, Group::Waiting);
        assert!(projected[1].reason.contains("Needs credentials"));
    }
    #[test]
    fn another_live_holder_is_found_by_work_item_reference() {
        let mut i = item("held", "issue", "in_progress", 1, 0);
        i.detail.assignment = Some(Assignment {
            id: "idle".into(),
            state: "idle".into(),
            ..Default::default()
        });
        let mut doc = document(vec![i]);
        doc.hosted.as_mut().unwrap().backlog.assignments.push(Assignment {
            id: "live".into(),
            state: "queued".into(),
            work_item: Some(WorkRef::Item { id: "held".into() }),
            ..Default::default()
        });
        assert_eq!(rows([&doc])[0].group, Group::InProgress);
    }
    #[test]
    fn work_label_combines_issue_and_pr_in_one_line() {
        let mut pr = item("pr", "pr", "ready", 1, 0);
        pr.number = 2;
        pr.detail.links.push(Link {
            kind: "closes".into(),
            to: Some("issue".into()),
            by: None,
        });
        let mut doc = document(vec![item("issue", "issue", "ready", 1, 0), pr]);
        doc.pull_requests
            .open
            .push(crate::fixtures::document(crate::fixtures::ATTENTION).pull_requests.open[0].clone());
        doc.pull_requests.open[0].number = 2;
        doc.pull_requests.open[0].state = "open".into();
        doc.pull_requests.open[0].review = "approved".into();
        assert!(
            rows([&doc])[0]
                .work_label()
                .starts_with("#1 issue → PR #2 (open, approved")
        );
    }

    #[test]
    fn waiting_reasons_only_come_from_waiting_members() {
        let mut issue = item("blocked", "issue", "blocked", 1, 0);
        issue.detail.status_reason = Some("Needs access".into());
        let mut pr = item("ready", "pr", "ready", 1, 0);
        pr.detail.status_reason = Some("Ready for merge".into());
        pr.detail.links.push(Link {
            kind: "closes".into(),
            to: Some(issue.id.clone()),
            by: None,
        });
        let mut doc = document(vec![issue, pr]);
        doc.hosted.as_mut().unwrap().backlog.plan.skipped.push(Decision {
            work_item: Ref { id: "ready".into() },
            reason: "Ready dispatch reason".into(),
        });
        let row = &rows([&doc])[0];
        assert_eq!(row.group, Group::Waiting);
        assert_eq!(row.reason, "Needs access · blocked");
    }

    #[test]
    fn up_next_omits_dispatch_reasons_and_opaque_thread_ids() {
        let mut i = item("next", "issue", "ready", 1, 0);
        i.detail.status_reason = Some("one writer per repo: thread thread:delegated-task:command%3Amcp%3Aid".into());
        let mut doc = document(vec![i]);
        doc.hosted.as_mut().unwrap().backlog.plan.skipped.push(Decision {
            work_item: Ref { id: "next".into() },
            reason: "one writer per repo: thread thread:delegated-task:command%3Amcp%3Aid".into(),
        });
        let row = &rows([&doc])[0];
        assert_eq!(row.group, Group::UpNext);
        assert!(row.reason.is_empty());
    }
    #[test]
    fn waiting_labels_have_one_human_spelling_and_no_thread_ids() {
        let mut i = item("waiting", "issue", "waiting_on_franz", 1, 0);
        i.detail.labels = vec!["waiting-on-franz".into(), "waiting_on_franz".into()];
        i.detail.status_reason = Some("waiting on franz · waiting-on-franz · thread:delegated-task:opaque".into());
        assert_eq!(rows([&document(vec![i])])[0].reason, "waiting on Franz");
    }
}
