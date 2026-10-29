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
    pub holds: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Context {
    pub assignments: Vec<Assignment>,
    pub plan: Plan,
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
    priority: u8,
    rank: i64,
    dispatch: usize,
}
impl Row {
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
            let mut reasons = Vec::new();
            let mut reviews = Vec::new();
            let mut active = false;
            let mut waiting = false;
            let mut dispatch = usize::MAX;
            for item in &items {
                let d = &item.detail;
                waiting |= d.assignment.as_ref().is_some_and(|a| {
                    matches!(a.state.as_str(), "blocked" | "watching" | "parked" | "waiting_on_franz")
                });
                waiting |= matches!(
                    item.status.as_str(),
                    "waiting_on_franz" | "blocked" | "watching" | "parked"
                );
                active |= item.status == "in_progress";
                if let Some(reason) = &d.status_reason {
                    reasons.push(reason.clone());
                }
                if waiting {
                    reasons.push(item.status.replace('_', " "));
                }
                if let Some(a) = &d.assignment {
                    active = true;
                    let model = hosted
                        .backlog
                        .assignments
                        .iter()
                        .find(|b| b.id == a.id)
                        .map(|b| b.model.as_str())
                        .unwrap_or(&a.model);
                    workers.push(format!("{} · {}", a.machine, model));
                }
                for label in &d.labels {
                    if label.starts_with("model:") {
                        workers.push(label.clone());
                    }
                }
                if let Some(pr) = doc
                    .pull_requests
                    .open
                    .iter()
                    .find(|p| item.kind == "pr" && p.number == item.number)
                {
                    reviews.push(format!(
                        "PR #{} · {} · {} · {}",
                        pr.number, pr.state, pr.review, pr.ai_review
                    ));
                    active = true;
                }
                if item.kind == "pr" && item.status == "ready" {
                    active = true;
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
                if let Some(d) = hosted.backlog.plan.skipped.iter().find(|d| d.work_item.id == item.id) {
                    reasons.push(d.reason.clone());
                }
            }
            workers.sort();
            workers.dedup();
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
            } else if items.iter().any(|i| i.status == "ready") {
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
    fn closing_links_group_transitively_and_keep_unrelated_items() {
        let issue = item("issue", "issue", "ready", 3, 0);
        let mut pr = item("pr", "pr", "in_progress", 3, 0);
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
        let doc = document(vec![
            item("later", "issue", "ready", 4, 0),
            item("wait", "issue", "blocked", 0, 0),
            item("p3", "issue", "ready", 3, 0),
            item("rank2", "issue", "ready", 0, 2),
            item("rank1", "issue", "ready", 0, 1),
            item("active", "pr", "in_progress", 3, 0),
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
            model: "claude".into(),
            ..Default::default()
        });
        let workers = &rows([&doc])[0].workers;
        assert!(workers.contains(&"model:gpt-6.1-sol".into()));
        assert!(workers.iter().any(|w| w.contains("claude")));
    }
}
